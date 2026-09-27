//! Slack-compatible incoming webhooks.
//!
//! Sideporch keeps a stable subset of Slack's payload: `text`,
//! `attachments` (with `fields`), and the `channel`, `username`, `icon_url`
//! and `icon_emoji` overrides that Mattermost and Gatus send. Unknown fields
//! are ignored, and `null` is accepted wherever a value is optional.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// A webhook request body as sent by Slack-compatible tools.
#[derive(Debug, Default, Deserialize)]
pub struct Payload {
    #[serde(default, deserialize_with = "lenient_string")]
    pub text: Option<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    pub attachments: Vec<IncomingAttachment>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub channel: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub username: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub icon_url: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub icon_emoji: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct IncomingAttachment {
    #[serde(default, deserialize_with = "lenient_string")]
    fallback: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    color: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pretext: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    author_name: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    title_link: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    text: Option<String>,
    #[serde(default, deserialize_with = "lenient_list")]
    fields: Vec<IncomingField>,
    #[serde(default, deserialize_with = "lenient_string")]
    footer: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct IncomingField {
    #[serde(default, deserialize_with = "lenient_string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    value: Option<String>,
    #[serde(default)]
    short: Option<bool>,
}

/// The attachment as Sideporch stores and renders it. Text fields hold
/// Slack markup; `color` and `title_link` are already validated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pretext: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
}

impl Attachment {
    /// The attachment's text, for the search index.
    pub fn searchable_text(&self) -> Vec<String> {
        [
            &self.pretext,
            &self.author_name,
            &self.title,
            &self.text,
            &self.footer,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .chain(
            self.fields
                .iter()
                .flat_map(|field| [field.title.clone(), field.value.clone()]),
        )
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub title: String,
    pub value: String,
    pub short: bool,
}

/// A validated webhook message, ready to store.
#[derive(Debug, PartialEq, Eq)]
pub struct WebhookMessage {
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub channel: Option<String>,
    pub username: Option<String>,
    pub icon_url: Option<String>,
}

/// Why a payload was rejected. The codes match Slack's error responses.
#[derive(Debug, PartialEq, Eq)]
pub enum Rejection {
    InvalidPayload,
    NoText,
}

impl Rejection {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidPayload => "invalid_payload",
            Self::NoText => "no_text",
        }
    }
}

/// Parses a JSON body, or a form body carrying JSON in its `payload`
/// field as Slack's older integrations send it.
pub fn parse(content_type: Option<&str>, body: &[u8]) -> Result<WebhookMessage, Rejection> {
    let is_form = content_type.is_some_and(|value| {
        value
            .trim()
            .to_ascii_lowercase()
            .starts_with("application/x-www-form-urlencoded")
    });
    let payload: Payload = if is_form {
        let json = form_payload(body).ok_or(Rejection::InvalidPayload)?;
        serde_json::from_str(&json).map_err(|_| Rejection::InvalidPayload)?
    } else {
        serde_json::from_slice(body).map_err(|_| Rejection::InvalidPayload)?
    };
    normalize(payload)
}

fn normalize(payload: Payload) -> Result<WebhookMessage, Rejection> {
    let text = payload
        .text
        .map(|text| text.trim().to_owned())
        .unwrap_or_default();
    let attachments: Vec<Attachment> = payload
        .attachments
        .into_iter()
        .filter_map(normalize_attachment)
        .collect();
    if text.is_empty() && attachments.is_empty() {
        return Err(Rejection::NoText);
    }
    let icon_url = payload
        .icon_url
        .filter(|url| is_http_url(url))
        .or_else(|| payload.icon_emoji.map(|emoji| emoji.trim().to_owned()))
        .filter(|icon| !icon.is_empty());
    Ok(WebhookMessage {
        text,
        attachments,
        channel: non_empty(
            payload
                .channel
                .map(|c| c.trim().trim_start_matches('#').to_owned()),
        ),
        username: non_empty(payload.username.map(|name| clip(name.trim(), 80))),
        icon_url,
    })
}

fn normalize_attachment(incoming: IncomingAttachment) -> Option<Attachment> {
    let fields: Vec<Field> = incoming
        .fields
        .into_iter()
        .filter_map(|field| {
            let title = field.title.unwrap_or_default();
            let value = field.value.unwrap_or_default();
            (!title.is_empty() || !value.is_empty()).then(|| Field {
                title,
                value: value.trim_end().to_owned(),
                short: field.short.unwrap_or(false),
            })
        })
        .collect();
    let text = non_empty(incoming.text).or_else(|| {
        // Show the fallback only when nothing else would describe the event.
        (incoming.title.is_none() && fields.is_empty()).then_some(incoming.fallback)?
    });
    let attachment = Attachment {
        color: incoming.color.as_deref().and_then(parse_color),
        pretext: non_empty(incoming.pretext),
        author_name: non_empty(incoming.author_name),
        title: non_empty(incoming.title),
        title_link: incoming.title_link.filter(|url| is_http_url(url)),
        text: non_empty(text),
        fields,
        footer: non_empty(incoming.footer),
    };
    let empty = attachment.pretext.is_none()
        && attachment.author_name.is_none()
        && attachment.title.is_none()
        && attachment.text.is_none()
        && attachment.fields.is_empty()
        && attachment.footer.is_none();
    (!empty).then_some(attachment)
}

/// Accepts `#rgb`, `#rrggbb` (with or without `#`) and Slack's named
/// colours. Returns a normalized `#rrggbb` value that is safe in CSS.
pub fn parse_color(value: &str) -> Option<String> {
    let value = value.trim();
    match value.to_ascii_lowercase().as_str() {
        "good" => return Some("#2EB67D".to_owned()),
        "warning" => return Some("#ECB22E".to_owned()),
        "danger" => return Some("#E01E5A".to_owned()),
        _ => {}
    }
    let hex = value.strip_prefix('#').unwrap_or(value);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => Some(format!("#{}", hex.to_ascii_uppercase())),
        3 => Some(
            hex.chars()
                .flat_map(|c| [c, c])
                .fold(String::from("#"), |mut out, c| {
                    out.push(c.to_ascii_uppercase());
                    out
                }),
        ),
        _ => None,
    }
}

fn is_http_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn clip(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn form_payload(body: &[u8]) -> Option<String> {
    let body = std::str::from_utf8(body).ok()?;
    body.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "payload").then(|| percent_decode(value))?
    })
}

fn percent_decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut iter = value.bytes();
    while let Some(byte) = iter.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = char::from(iter.next()?).to_digit(16)?;
                let low = char::from(iter.next()?).to_digit(16)?;
                bytes.push(u8::try_from(high.checked_mul(16)?.checked_add(low)?).ok()?);
            }
            other => bytes.push(other),
        }
    }
    String::from_utf8(bytes).ok()
}

/// Accepts a string, number or boolean as text; treats `null` as absent.
fn lenient_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => Some(text),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    })
}

/// Treats `null` as an empty list.
fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("#dd0000").as_deref(), Some("#DD0000"));
        assert_eq!(parse_color("36a64f").as_deref(), Some("#36A64F"));
        assert_eq!(parse_color("#abc").as_deref(), Some("#AABBCC"));
        assert_eq!(parse_color("danger").as_deref(), Some("#E01E5A"));
        assert_eq!(parse_color("red; background: url(x)"), None);
        assert_eq!(parse_color("#12345"), None);
    }

    #[test]
    fn rejects_empty_and_invalid_payloads() {
        assert_eq!(parse(None, br#"{"text": "  "}"#), Err(Rejection::NoText));
        assert_eq!(parse(None, b"not json"), Err(Rejection::InvalidPayload));
        assert_eq!(
            parse(None, br#"{"attachments": [{}]}"#),
            Err(Rejection::NoText)
        );
    }

    #[test]
    fn accepts_form_encoded_payloads() {
        let body = b"payload=%7B%22text%22%3A%22hello+world%22%7D";
        let message = parse(Some("application/x-www-form-urlencoded"), body).unwrap();
        assert_eq!(message.text, "hello world");
    }

    #[test]
    fn ignores_unknown_fields_and_nulls() {
        let body =
            br##"{"text": null, "blocks": [], "icon_url": "javascript:x", "icon_emoji": ":ghost:",
            "attachments": [{"title": "T", "fields": null, "ts": 123, "color": "#zzzzzz"}]}"##;
        let message = parse(Some("application/json"), body).unwrap();
        assert_eq!(message.text, "");
        assert_eq!(message.icon_url.as_deref(), Some(":ghost:"));
        let attachment = message.attachments.first().unwrap();
        assert_eq!(attachment.title.as_deref(), Some("T"));
        assert_eq!(attachment.color, None);
        assert!(attachment.fields.is_empty());
    }

    #[test]
    fn uses_fallback_only_when_nothing_else_describes_the_attachment() {
        let body = br#"{"attachments": [{"fallback": "only this"}]}"#;
        let message = parse(None, body).unwrap();
        assert_eq!(
            message.attachments.first().unwrap().text.as_deref(),
            Some("only this")
        );
    }
}
