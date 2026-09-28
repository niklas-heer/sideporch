//! Optional AI help for writing automations.
//!
//! An admin connects a provider: Anthropic's Messages API, or any API that
//! speaks the chat completions format `OpenAI` introduced, which `OpenRouter`,
//! Ollama and many others share. Sideporch sends the request, the automation API reference
//! and the current script, then lints and test-loads the answer and sends
//! any problems back once or twice before returning a draft for review.

use std::time::Duration;

use axum::body::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    automations::{self, TestTrigger, api, tooling},
    error::{AppError, AppResult},
    store,
};

const TIMEOUT: Duration = Duration::from_secs(240);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
/// Output budget for one answer; scripts are short, but thinking counts too.
const MAX_TOKENS: u32 = 16_000;
/// How often Sideporch sends problems back before giving up.
const REPAIR_ROUNDS: usize = 2;

type HttpClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Anthropic,
    OpenAi,
}

impl Protocol {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            _ => None,
        }
    }

    pub const fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAi => "https://api.openai.com/v1",
        }
    }
}

/// A connected AI provider, as the admin configured it.
#[derive(Debug, Clone)]
pub struct Provider {
    pub protocol: Protocol,
    pub base_url: String,
    pub model: String,
    /// Empty for local servers such as Ollama.
    pub api_key: String,
}

impl Provider {
    /// The last four characters of the key, for the settings page.
    pub fn key_hint(&self) -> Option<String> {
        let count = self.api_key.chars().count();
        (count > 0).then(|| self.api_key.chars().skip(count.saturating_sub(4)).collect())
    }
}

const PROTOCOL: &str = "ai.protocol";
const BASE_URL: &str = "ai.base_url";
const MODEL: &str = "ai.model";
const API_KEY: &str = "ai.api_key";

pub fn provider(conn: &Connection) -> AppResult<Option<Provider>> {
    let Some(protocol) = store::setting(conn, PROTOCOL)?.and_then(|key| Protocol::from_key(&key))
    else {
        return Ok(None);
    };
    Ok(Some(Provider {
        protocol,
        base_url: store::setting(conn, BASE_URL)?
            .unwrap_or_else(|| protocol.default_base_url().to_owned()),
        model: store::setting(conn, MODEL)?.unwrap_or_default(),
        api_key: store::setting(conn, API_KEY)?.unwrap_or_default(),
    }))
}

pub fn save_provider(conn: &Connection, provider: &Provider) -> AppResult<()> {
    store::set_setting(conn, PROTOCOL, provider.protocol.key())?;
    store::set_setting(conn, BASE_URL, &provider.base_url)?;
    store::set_setting(conn, MODEL, &provider.model)?;
    store::set_setting(conn, API_KEY, &provider.api_key)
}

pub fn remove_provider(conn: &Connection) -> AppResult<()> {
    for key in [PROTOCOL, BASE_URL, MODEL, API_KEY] {
        store::delete_setting(conn, key)?;
    }
    Ok(())
}

/// Sends requests to AI providers.
pub struct Ai {
    client: HttpClient,
}

impl Ai {
    pub fn new() -> AppResult<Self> {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
            .map_err(AppError::internal)?
            // Local servers such as Ollama speak plain HTTP.
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
        })
    }

    async fn post(&self, url: &str, headers: &[(&str, &str)], body: &Value) -> AppResult<Value> {
        let mut request = axum::http::Request::post(url).header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let request = request
            .body(Full::new(Bytes::from(body.to_string())))
            .map_err(|error| {
                AppError::bad_request(format!("The AI provider URL is invalid: {error}"))
            })?;
        let response = tokio::time::timeout(TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| AppError::bad_request("The AI provider did not answer in time."))?
            .map_err(|error| {
                AppError::bad_request(format!("Could not reach the AI provider: {error}"))
            })?;
        let status = response.status();
        let bytes = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|error| {
                AppError::bad_request(format!("The AI provider's answer broke off: {error}"))
            })?
            .to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !status.is_success() {
            let detail = value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map_or_else(
                    || String::from_utf8_lossy(&bytes).chars().take(300).collect(),
                    ToOwned::to_owned,
                );
            return Err(AppError::bad_request(format!(
                "The AI provider answered {status}: {detail}"
            )));
        }
        Ok(value)
    }

    /// One reply to a conversation of alternating user and assistant turns.
    async fn chat(&self, provider: &Provider, system: &str, turns: &[Turn]) -> AppResult<String> {
        let base = provider.base_url.trim_end_matches('/');
        match provider.protocol {
            Protocol::Anthropic => {
                let messages: Vec<Value> = turns
                    .iter()
                    .map(|turn| json!({ "role": turn.role, "content": turn.text }))
                    .collect();
                let mut body = json!({
                    "model": provider.model,
                    "max_tokens": MAX_TOKENS,
                    "system": system,
                    "messages": messages,
                });
                let mut headers = vec![
                    ("x-api-key", provider.api_key.as_str()),
                    ("anthropic-version", "2023-06-01"),
                ];
                // Models with safety classifiers can decline; let Anthropic
                // retry on its recommended fallback model instead.
                if has_server_fallbacks(&provider.model)
                    && let Some(fields) = body.as_object_mut()
                {
                    fields.insert("fallbacks".to_owned(), json!("default"));
                    headers.push(("anthropic-beta", "server-side-fallback-2026-07-01"));
                }
                let answer = self
                    .post(&format!("{base}/v1/messages"), &headers, &body)
                    .await?;
                if answer.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
                    return Err(AppError::bad_request(
                        "The model declined this request. Try rephrasing it.",
                    ));
                }
                let text: Vec<&str> = answer
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect();
                Ok(text.join(""))
            }
            Protocol::OpenAi => {
                let mut messages = vec![json!({ "role": "system", "content": system })];
                messages.extend(
                    turns
                        .iter()
                        .map(|turn| json!({ "role": turn.role, "content": turn.text })),
                );
                let body = json!({ "model": provider.model, "messages": messages });
                let authorization = format!("Bearer {}", provider.api_key);
                let headers: Vec<(&str, &str)> = if provider.api_key.is_empty() {
                    Vec::new()
                } else {
                    vec![("authorization", authorization.as_str())]
                };
                let answer = self
                    .post(&format!("{base}/chat/completions"), &headers, &body)
                    .await?;
                answer
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .ok_or_else(|| AppError::bad_request("The AI provider sent no answer."))
            }
        }
    }
}

/// Anthropic models that accept `fallbacks: "default"`.
fn has_server_fallbacks(model: &str) -> bool {
    ["claude-opus-5", "claude-opus-5-5", "claude-fable-5-1"].contains(&model)
}

struct Turn {
    role: &'static str,
    text: String,
}

/// What someone asked for, with the context the model needs.
pub struct ScriptRequest {
    pub prompt: String,
    pub name: String,
    pub source: String,
    pub channels: Vec<String>,
}

/// A proposed script, for the admin to review.
#[derive(Debug, Serialize)]
pub struct Draft {
    pub source: String,
    pub explanation: String,
    /// Problems still left after the repair rounds.
    pub diagnostics: Vec<tooling::Diagnostic>,
    pub load_error: Option<String>,
}

fn system_prompt() -> String {
    format!(
        "You write automations for Sideporch, a self-hosted team chat. Automations are Lua 5.4 scripts \
that run in a sandbox inside the server and use only the API below.\n\n{}\n\n\
Rules:\n\
- Answer with the complete script in one ```lua code block, then at most three short sentences \
that explain what it does and anything the admin must set up (such as a channel or the webhook URL).\n\
- Register handlers at the top level; the top level runs once when the script loads.\n\
- Use `local` variables, handle missing fields and bad input without crashing, and keep the \
script short and readable, with a comment above each handler.\n\
- Refer to channels by name without `#`. Only use channels that exist, unless the request names another.\n\
- There is no network, file, or OS access; if a request needs one, say so and do what is possible.",
        api::reference()
    )
}

fn user_prompt(request: &ScriptRequest) -> String {
    let channels = if request.channels.is_empty() {
        "none yet".to_owned()
    } else {
        request.channels.join(", ")
    };
    let current = if request.source.trim().is_empty() {
        "The script is empty.".to_owned()
    } else {
        format!("The current script:\n```lua\n{}\n```", request.source)
    };
    let name = if request.name.trim().is_empty() {
        "not named yet"
    } else {
        request.name.as_str()
    };
    format!(
        "Automation name: {name}\nPublic channels: {channels}\n\n{current}\n\nRequest: {}",
        request.prompt
    )
}

/// The first fenced code block of `answer`, and the text around it.
fn split_answer(answer: &str) -> (Option<String>, String) {
    let Some(start) = answer.find("```") else {
        return (None, answer.trim().to_owned());
    };
    let before = answer.get(..start).unwrap_or_default();
    let after_fence = answer.get(start.saturating_add(3)..).unwrap_or_default();
    // Skip the language tag on the opening line.
    let body_start = after_fence.find('\n').map_or(0, |at| at.saturating_add(1));
    let body = after_fence.get(body_start..).unwrap_or_default();
    let Some(end) = body.find("```") else {
        return (Some(body.trim_end().to_owned()), before.trim().to_owned());
    };
    let code = body.get(..end).unwrap_or_default().trim_end().to_owned();
    let after = body.get(end.saturating_add(3)..).unwrap_or_default();
    let explanation = format!("{} {}", before.trim(), after.trim())
        .trim()
        .to_owned();
    (Some(code), explanation)
}

/// Lints the script and runs its top level in a dry run.
async fn check(source: String) -> AppResult<(Vec<tooling::Diagnostic>, Option<String>)> {
    tokio::task::spawn_blocking(move || {
        let diagnostics = tooling::lint(&source);
        let load_error = if tooling::has_errors(&diagnostics) {
            None
        } else {
            automations::test(
                "Automation",
                &source,
                std::collections::HashMap::new(),
                &TestTrigger::Load,
            )
            .error
        };
        (diagnostics, load_error)
    })
    .await
    .map_err(AppError::internal)
}

fn problems(diagnostics: &[tooling::Diagnostic], load_error: Option<&str>) -> Option<String> {
    let mut lines: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
    if let Some(error) = load_error {
        lines.push(format!("Loading the script failed: {error}"));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Asks the provider for a script, then sends lint and load problems back
/// until the script is clean or the repair rounds run out.
pub async fn write_script(
    ai: &Ai,
    provider: &Provider,
    request: &ScriptRequest,
) -> AppResult<Draft> {
    let system = system_prompt();
    let mut turns = vec![Turn {
        role: "user",
        text: user_prompt(request),
    }];
    let mut round = 0;
    loop {
        let answer = ai.chat(provider, &system, &turns).await?;
        let (code, explanation) = split_answer(&answer);
        let Some(code) = code else {
            return Err(AppError::bad_request(format!(
                "The AI answered without a script: {}",
                explanation.chars().take(500).collect::<String>()
            )));
        };
        let (diagnostics, load_error) = check(code.clone()).await?;
        let feedback = problems(&diagnostics, load_error.as_deref());
        if feedback.is_none() || round >= REPAIR_ROUNDS {
            let source = if feedback.is_none() {
                tooling::format(&code).unwrap_or(code)
            } else {
                code
            };
            return Ok(Draft {
                source,
                explanation,
                diagnostics,
                load_error,
            });
        }
        round = round.saturating_add(1);
        turns.push(Turn {
            role: "assistant",
            text: answer,
        });
        turns.push(Turn {
            role: "user",
            text: format!(
                "Sideporch's linter and a test load found these problems:\n{}\n\nFix them and answer with the complete script again.",
                feedback.unwrap_or_default()
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_code_from_explanation() {
        let (code, explanation) =
            split_answer("Here you go:\n```lua\nprint(1)\n```\nIt prints one.");
        assert_eq!(code.as_deref(), Some("print(1)"));
        assert_eq!(explanation, "Here you go: It prints one.");
        let (code, _) = split_answer("```\nprint(2)\n```");
        assert_eq!(code.as_deref(), Some("print(2)"));
        assert_eq!(split_answer("no code").0, None);
    }

    #[test]
    fn the_prompt_carries_the_reference() {
        assert!(system_prompt().contains("sideporch.on_webhook(handler)"));
    }
}
