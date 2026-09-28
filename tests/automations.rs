// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! The automation workbench: linting, formatting and test runs from the
//! editor, reaction and webhook triggers, version history, the MCP server,
//! and AI help, driven through a real server.

mod common;

use std::sync::{Arc, Mutex};

use axum::{Json, Router, http::HeaderMap, routing::post};
use common::{Browser, admin, between, home_channel, invite, location, start};
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn post_json(browser: &Browser, path: &str, body: &Value) -> reqwest::Response {
    browser
        .client
        .post(browser.url(path))
        .header("cookie", &browser.cookie)
        .json(body)
        .send()
        .await
        .unwrap()
}

async fn json_ok(browser: &Browser, path: &str, body: &Value) -> Value {
    let response = post_json(browser, path, body).await;
    assert_eq!(response.status(), StatusCode::OK, "POST {path}");
    response.json().await.unwrap()
}

async fn save(admin: &Browser, name: &str, source: &str) -> String {
    let response = admin
        .post(
            "/automations",
            &[("name", name), ("source", source), ("enabled", "on")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    location(&response)
}

fn unescape(html: &str) -> String {
    html.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[tokio::test]
async fn the_editor_lints_formats_and_test_runs_scripts() {
    let server = start().await;
    let admin = admin(&server).await;

    let lint = json_ok(
        &admin,
        "/automations/lint",
        &json!({ "source": "local unused = os.time()\nsideporch.pots('general', 'hi')" }),
    )
    .await;
    let codes: Vec<&str> = lint["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"unused_variable"), "{lint}");
    assert!(codes.contains(&"undefined_variable"), "{lint}");
    assert!(codes.contains(&"incorrect_standard_library_use"), "{lint}");
    let os = lint["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "undefined_variable")
        .unwrap();
    assert_eq!(
        (os["line"].as_i64(), os["column"].as_i64()),
        (Some(1), Some(16))
    );
    assert!(os["message"].as_str().unwrap().contains("sandbox"));

    let formatted = json_ok(
        &admin,
        "/automations/format",
        &json!({ "source": "if true then print('x') end" }),
    )
    .await;
    assert_eq!(formatted["source"], "if true then\n  print(\"x\")\nend\n");
    let broken = json_ok(
        &admin,
        "/automations/format",
        &json!({ "source": "if then" }),
    )
    .await;
    assert!(broken["source"].is_null());
    assert_eq!(broken["diagnostics"][0]["code"], "syntax");

    let report = json_ok(
        &admin,
        "/automations/test",
        &json!({
            "source": "sideporch.on_message(function(msg)\n  print('heard', msg.text)\n  if msg.text == '!ping' then sideporch.reply(msg, 'pong') end\nend)",
            "trigger": { "kind": "message", "text": "!ping", "channel": "general" }
        }),
    )
    .await;
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(report["called"], 1);
    assert_eq!(
        report["log"],
        json!(["heard\t!ping", "→ post in #general (thread 1): pong"])
    );

    let failing = json_ok(
        &admin,
        "/automations/test",
        &json!({
            "source": "sideporch.on_webhook(function(req)\n  return req.json.missing.field\nend)",
            "trigger": { "kind": "webhook", "body": "{}" }
        }),
    )
    .await;
    assert_eq!(failing["ok"], false);
    assert!(
        failing["error"].as_str().unwrap().starts_with("line 2: "),
        "{failing}"
    );

    // Nothing was saved, and people who aren't admins get no access.
    assert!(
        admin
            .page("/automations")
            .await
            .contains("No automations yet")
    );
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    for path in [
        "/automations/lint",
        "/automations/format",
        "/automations/test",
    ] {
        let response = post_json(&member, path, &json!({ "source": "" })).await;
        assert!(
            [StatusCode::FORBIDDEN, StatusCode::UNPROCESSABLE_ENTITY].contains(&response.status()),
            "{path}"
        );
    }
    assert_eq!(
        post_json(
            &member,
            "/automations/lint",
            &json!({ "source": "print(1)" })
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn automations_react_to_reactions() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let path = save(
        &admin,
        "Cheerleader",
        r#"sideporch.on_reaction(function(e)
  if e.added and e.emoji == "tada" then
    sideporch.react(e.message, "eyes")
    sideporch.reply(e.message, e.user .. " cheered for " .. e.message.author)
  end
end)"#,
    )
    .await;

    assert_eq!(
        admin.send(general, "We shipped it", None).await,
        StatusCode::NO_CONTENT
    );
    let page = admin.page(&format!("/c/{general}")).await;
    let message = common::last_message_id(&page);
    let response = admin
        .post(
            &format!("/c/{general}/m/{message}/reactions"),
            &[("emoji", "tada")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let thread = admin
        .wait_for(
            &format!("/c/{general}/t/{message}"),
            "Ada Admin cheered for Ada Admin",
        )
        .await;
    // The automation's reaction shows up next to the person's.
    assert!(
        thread.contains("Cheerleader reacted with :eyes:"),
        "{thread}"
    );
    assert!(thread.contains("Ada Admin reacted with :tada:"));

    // Removing a reaction is an event too, but this script ignores it.
    admin
        .post(
            &format!("/c/{general}/m/{message}/reactions"),
            &[("emoji", "tada")],
        )
        .await;
    let editor = admin.wait_for(&path, "→ react :eyes: to message").await;
    assert!(editor.contains(">reaction_added<"));
}

#[tokio::test]
async fn automations_answer_their_webhook() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let path = save(
        &admin,
        "Deploy bot",
        r#"sideporch.on_webhook(function(req)
  if req.method ~= "POST" then
    return { status = 405, body = "POST only" }
  end
  local service = req.json and req.json.service or "something"
  sideporch.post("general", "Deploying **" .. service .. "** (" .. req.path .. ", token " .. tostring(req.headers["x-token"]) .. ")")
  return { json = { ok = true, service = service } }
end)"#,
    )
    .await;
    let editor = admin.page(&path).await;
    let url = between(&editor, "curl -X POST ", " ").to_owned();
    assert!(url.contains("/hooks/automations/"), "{url}");

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{url}/deploy"))
        .header("x-token", "s3cret")
        .json(&json!({ "service": "garden" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({ "ok": true, "service": "garden" })
    );
    admin
        .wait_for(
            &format!("/c/{general}"),
            "Deploying <strong>garden</strong> (/deploy, token s3cret)",
        )
        .await;

    let wrong = client.get(&url).send().await.unwrap();
    assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(wrong.text().await.unwrap(), "POST only");

    // Each request lands in the run log.
    let editor = admin.page(&path).await;
    assert!(editor.contains("POST /deploy"));
    assert!(editor.contains("GET /"));

    // A new URL replaces the old one.
    let rotated = admin.post(&format!("{path}/webhook-token"), &[]).await;
    assert_eq!(rotated.status(), StatusCode::SEE_OTHER);
    let old = client.post(&url).send().await.unwrap();
    assert_eq!(old.status(), StatusCode::NOT_FOUND);

    // Scripts without a handler, or switched off, say so.
    let quiet = save(&admin, "Quiet", "-- nothing").await;
    let quiet_url = between(&admin.page(&quiet).await, "curl -X POST ", " ").to_owned();
    assert_eq!(
        client.post(&quiet_url).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    admin
        .post(&quiet, &[("name", "Quiet"), ("source", "-- nothing")])
        .await;
    assert_eq!(
        client.post(&quiet_url).send().await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn saves_keep_a_history_that_can_be_restored() {
    let server = start().await;
    let admin = admin(&server).await;
    let path = save(&admin, "Greeter", "print('first')").await;
    admin
        .post(
            &path,
            &[
                ("name", "Greeter"),
                ("source", "print('second')"),
                ("enabled", "on"),
            ],
        )
        .await;
    let page = unescape(&admin.page(&path).await);
    assert!(page.contains("print('second')"));
    let history = between(&page, "Every saved version", "What scripts can do");
    assert_eq!(history.matches("with editor").count(), 2);
    let first = between(history, "print('first')", "Restore this version");
    let restore = between(first, "action=\"", "\"").to_owned();
    assert!(restore.ends_with("/restore"), "{restore}");
    assert_eq!(
        admin.post(&restore, &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    let page = unescape(&admin.page(&path).await);
    assert!(between(&page, "<textarea", "</textarea>").contains("print('first')"));
    assert!(page.contains("with restore"));
}

/// Creates an MCP token on the settings page and returns it.
async fn mcp_token(admin: &Browser) -> String {
    let response = admin
        .post("/settings/mcp/tokens", &[("name", "Test agent")])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = response.text().await.unwrap();
    format!("sp_{}", between(&page, "data-copy=\"sp_", "\""))
}

async fn rpc(base: &str, token: &str, method: &str, params: Value) -> Value {
    let response = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .bearer_auth(token)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn call(base: &str, token: &str, tool: &str, arguments: Value) -> Value {
    rpc(
        base,
        token,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
    .await["result"]
        .clone()
}

#[tokio::test]
async fn agents_connect_over_mcp() {
    let server = start().await;
    let admin = admin(&server).await;
    let base = &server.base;

    let anonymous = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let token = mcp_token(&admin).await;
    let init = rpc(
        base,
        &token,
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }),
    )
    .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("sideporch.on_reaction")
    );
    let notified = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await
        .unwrap();
    assert_eq!(notified.status(), StatusCode::ACCEPTED);

    let tools = rpc(base, &token, "tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for expected in [
        "lint_lua",
        "test_automation",
        "save_automation",
        "list_runs",
    ] {
        assert!(names.contains(&expected), "{names:?}");
    }
}

#[tokio::test]
async fn agents_write_test_and_save_automations_over_mcp() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let base = &server.base;
    let token = mcp_token(&admin).await;

    // Broken scripts are refused.
    let refused = call(
        base,
        &token,
        "save_automation",
        json!({ "name": "Broken", "source": "sideporch.post(" }),
    )
    .await;
    assert_eq!(refused["isError"], true);
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("syntax")
    );

    let source = "-- Answers !ping\nsideporch.on_message(function(msg)\n  if msg.text == \"!ping\" then\n    sideporch.reply(msg, \"pong from MCP\")\n  end\nend)\n";
    let tested = call(
        base,
        &token,
        "test_automation",
        json!({ "source": source, "trigger": { "kind": "message", "text": "!ping" } }),
    )
    .await;
    assert_eq!(tested["structuredContent"]["ok"], true, "{tested}");

    let saved = call(
        base,
        &token,
        "save_automation",
        json!({ "name": "MCP pinger", "source": source }),
    )
    .await;
    let id = saved["structuredContent"]["id"].as_i64().unwrap();
    // New automations start switched off.
    assert_eq!(saved["structuredContent"]["enabled"], false);
    let listed = call(base, &token, "list_automations", json!({})).await;
    assert_eq!(
        listed["structuredContent"]["automations"][0]["name"],
        "MCP pinger"
    );

    let enabled = call(
        base,
        &token,
        "save_automation",
        json!({ "id": id, "name": "MCP pinger", "source": source, "enabled": true }),
    )
    .await;
    assert_eq!(enabled["structuredContent"]["enabled"], true);
    admin.send(general, "!ping", None).await;
    let ping = common::last_message_id(&admin.page(&format!("/c/{general}")).await);
    admin
        .wait_for(&format!("/c/{general}/t/{ping}"), "pong from MCP")
        .await;

    // The history names the token that saved it, and the token was used.
    let editor = admin.page(&format!("/automations/{id}")).await;
    assert!(editor.contains("with MCP: Test agent"), "{editor}");
    assert!(admin.page("/settings/mcp").await.contains("last used"));

    let deleted = call(base, &token, "delete_automation", json!({ "id": id })).await;
    assert_eq!(deleted["structuredContent"]["deleted"], id);
    let unknown = rpc(base, &token, "tools/call", json!({ "name": "nope" })).await;
    assert_eq!(unknown["error"]["code"], -32602);

    // Revoked tokens stop working.
    let settings = admin.page("/settings/mcp").await;
    let revoke = between(&settings, "action=\"/settings/mcp/tokens/", "\"");
    admin
        .post(&format!("/settings/mcp/tokens/{revoke}"), &[])
        .await;
    let revoked = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

/// A fake chat completions API that answers with the queued replies and
/// records every request.
async fn fake_ai(replies: Vec<&'static str>) -> (String, Arc<Mutex<Vec<(HeaderMap, Value)>>>) {
    let seen: Arc<Mutex<Vec<(HeaderMap, Value)>>> = Arc::default();
    let replies = Arc::new(Mutex::new(replies));
    let record = Arc::clone(&seen);
    let queue = Arc::clone(&replies);
    let openai = move |headers: HeaderMap, Json(body): Json<Value>| {
        let record = Arc::clone(&record);
        let queue = Arc::clone(&queue);
        async move {
            record.lock().unwrap().push((headers, body));
            let reply = queue.lock().unwrap().remove(0);
            Json(json!({ "choices": [{ "message": { "role": "assistant", "content": reply } }] }))
        }
    };
    let record = Arc::clone(&seen);
    let queue = Arc::clone(&replies);
    let anthropic = move |headers: HeaderMap, Json(body): Json<Value>| {
        let record = Arc::clone(&record);
        let queue = Arc::clone(&queue);
        async move {
            record.lock().unwrap().push((headers, body));
            let reply = queue.lock().unwrap().remove(0);
            Json(json!({
                "stop_reason": "end_turn",
                "content": [{ "type": "thinking", "thinking": "" }, { "type": "text", "text": reply }]
            }))
        }
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(openai))
        .route("/v1/messages", post(anthropic));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

#[tokio::test]
async fn ai_writes_scripts_and_fixes_its_mistakes() {
    let server = start().await;
    let admin = admin(&server).await;
    let (ai, seen) = fake_ai(vec![
        "Here it is:\n```lua\nsideporch.on_message(function(msg)\n  sideporch.reply(msg, greeting)\nend)\n```",
        "Fixed.\n```lua\nsideporch.on_message(function(msg) if msg.text == \"hi\" then sideporch.reply(msg, \"hello\") end end)\n```\nIt answers hi.",
    ])
    .await;

    let editor = admin.page("/automations/new").await;
    assert!(editor.contains("No AI provider is connected yet"));
    let response = post_json(&admin, "/automations/ai", &json!({ "prompt": "greet" })).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let saved = admin
        .post(
            "/settings/ai",
            &[
                ("protocol", "openai"),
                ("base_url", &format!("{ai}/v1")),
                ("model", "local-coder"),
                ("api_key", "sk-test-1234"),
            ],
        )
        .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let settings = saved.text().await.unwrap();
    // The key is never shown again, only its end.
    assert!(!settings.contains("sk-test-1234"));
    assert!(settings.contains("ending in …1234"));
    assert!(
        admin
            .page("/automations/new")
            .await
            .contains("data-ai-form")
    );

    let draft = json_ok(
        &admin,
        "/automations/ai",
        &json!({ "prompt": "Answer hi with hello", "source": "", "name": "Greeter" }),
    )
    .await;
    assert_eq!(
        draft["source"],
        "sideporch.on_message(function(msg)\n  if msg.text == \"hi\" then\n    sideporch.reply(msg, \"hello\")\n  end\nend)\n"
    );
    assert_eq!(draft["explanation"], "Fixed. It answers hi.");
    assert_eq!(draft["diagnostics"], json!([]));

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "one repair round");
    let (headers, first) = &requests[0];
    assert_eq!(headers["authorization"], "Bearer sk-test-1234");
    assert_eq!(first["model"], "local-coder");
    assert!(
        first["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("sideporch.on_webhook(handler)")
    );
    assert!(
        first["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("Public channels: general")
    );
    let repair = requests[1].1["messages"][3]["content"].as_str().unwrap();
    assert!(repair.contains("greeting"), "{repair}");
}

#[tokio::test]
async fn ai_speaks_anthropics_api() {
    let server = start().await;
    let admin = admin(&server).await;
    // Anthropic's API gets its own headers and request shape.
    let (anthropic, seen) = fake_ai(vec!["```lua\nprint(\"hi\")\n```"]).await;
    admin
        .post(
            "/settings/ai",
            &[
                ("protocol", "anthropic"),
                ("base_url", &anthropic),
                ("model", "claude-opus-5"),
                ("api_key", "sk-ant-test"),
            ],
        )
        .await;
    let draft = json_ok(&admin, "/automations/ai", &json!({ "prompt": "say hi" })).await;
    assert_eq!(draft["source"], "print(\"hi\")\n");
    let (headers, body) = seen.lock().unwrap()[0].clone();
    assert_eq!(headers["x-api-key"], "sk-ant-test");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(headers["anthropic-beta"], "server-side-fallback-2026-07-01");
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["max_tokens"], 16_000);
    assert!(body["system"].as_str().unwrap().contains("Sideporch"));

    // Saving without a key keeps the stored one; disconnecting removes it.
    admin
        .post(
            "/settings/ai",
            &[
                ("protocol", "anthropic"),
                ("model", "claude-sonnet-5"),
                ("api_key", ""),
            ],
        )
        .await;
    assert!(admin.page("/settings/ai").await.contains("ending in …test"));
    admin.post("/settings/ai/remove", &[]).await;
    assert!(
        admin
            .page("/automations/new")
            .await
            .contains("No AI provider is connected yet")
    );
}
