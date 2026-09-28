//! Link previews: the title, description and image of the first link in a
//! message, fetched after it is posted.
//!
//! Fetching goes through the automations' guarded HTTP client, so a link
//! can never make the server reach its own network. Only HTML pages are
//! read, redirects are followed three times, and admins can turn previews
//! off. Images stay on the linked site; browsers load them from there.

use std::{sync::Arc, time::Duration};

use crate::{
    AppState,
    automations::http::{Http, Request},
    error::AppResult,
    messages, store,
};

const SETTING: &str = "previews.enabled";
const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_REDIRECTS: usize = 3;
const MAX_TITLE: usize = 200;
const MAX_DESCRIPTION: usize = 300;

pub use store::LinkPreview;

pub fn enabled(conn: &rusqlite::Connection) -> AppResult<bool> {
    Ok(store::setting(conn, SETTING)?.as_deref() != Some("false"))
}

pub fn set_enabled(conn: &rusqlite::Connection, enabled: bool) -> AppResult<()> {
    store::set_setting(conn, SETTING, if enabled { "true" } else { "false" })
}

/// The message text without code, where links are examples, not shares.
fn without_code(body: &str) -> String {
    let mut kept = String::new();
    let mut fenced = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            // Inline code: keep every other piece between backticks.
            for (index, piece) in line.split('`').enumerate() {
                if index % 2 == 0 {
                    kept.push_str(piece);
                    kept.push(' ');
                }
            }
            kept.push('\n');
        }
    }
    kept
}

/// The first http or https link in a message, outside code.
pub fn first_link(body: &str) -> Option<String> {
    let text = without_code(body);
    let start = text.find("https://").or_else(|| text.find("http://"))?;
    let rest = text.get(start..)?;
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '<' | '"' | '\'' | ']'))
        .unwrap_or(rest.len());
    let url = rest
        .get(..end)?
        .trim_end_matches(['.', ',', '!', '?', ';', ':', '*', '_']);
    (url.len() > "https://".len() && url.len() <= 2_000).then(|| url.to_owned())
}

/// Replaces the few HTML entities titles and descriptions use.
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(rest.get(..start).unwrap_or_default());
        let tail = rest.get(start..).unwrap_or_default();
        let Some(end) = tail.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = tail.get(1..).unwrap_or_default();
            continue;
        };
        let entity = tail.get(1..end).unwrap_or_default();
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        if let Some(c) = decoded {
            out.push(c);
            rest = tail.get(end.saturating_add(1)..).unwrap_or_default();
        } else {
            out.push('&');
            rest = tail.get(1..).unwrap_or_default();
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The value of `name` in an HTML tag's attributes.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower.get(from..)?.find(name) {
        let at = from.saturating_add(found);
        from = at.saturating_add(name.len());
        let before = lower.get(..at)?.chars().last();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let after = lower.get(from..)?.trim_start();
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let offset = lower.len().saturating_sub(value.len());
        let original = tag.get(offset..)?;
        let quote = original.chars().next()?;
        let content = if matches!(quote, '"' | '\'') {
            let inner = original.get(1..)?;
            inner.get(..inner.find(quote)?)?
        } else {
            original.get(
                ..original
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .unwrap_or(original.len()),
            )?
        };
        return Some(decode(content));
    }
    None
}

/// Makes `link` absolute against the page's `url`.
fn absolute(url: &str, link: &str) -> Option<String> {
    if link.starts_with("https://") || link.starts_with("http://") {
        return Some(link.to_owned());
    }
    let (scheme, rest) = url.split_once("://")?;
    if let Some(path) = link.strip_prefix("//") {
        return Some(format!("{scheme}://{path}"));
    }
    let host = rest.split(['/', '?', '#']).next()?;
    link.starts_with('/')
        .then(|| format!("{scheme}://{host}{link}"))
}

fn clip(text: &str, max: usize) -> String {
    let mut clipped: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        clipped.push('…');
    }
    clipped
}

/// Reads Open Graph and plain HTML metadata from a page.
pub fn parse_html(url: &str, html: &str) -> Option<LinkPreview> {
    let head = html.get(..html.len().min(512 * 1024)).unwrap_or(html);
    let mut meta = std::collections::HashMap::new();
    // Lowercasing ASCII keeps byte offsets, so positions carry over.
    let lower = head.to_ascii_lowercase();
    let mut position = 0;
    while let Some(found) = lower.get(position..).and_then(|rest| rest.find("<meta")) {
        let start = position.saturating_add(found);
        let tail = head.get(start..)?;
        let end = tail.find('>').unwrap_or(tail.len());
        let tag = tail.get(..end)?;
        let key = attribute(tag, "property").or_else(|| attribute(tag, "name"));
        if let (Some(key), Some(content)) = (key, attribute(tag, "content")) {
            meta.entry(key.to_ascii_lowercase()).or_insert(content);
        }
        position = start.saturating_add(end.max(1));
    }
    let title_tag = lower.find("<title").and_then(|start| {
        let open = start
            .saturating_add(lower.get(start..)?.find('>')?)
            .saturating_add(1);
        let close = open.saturating_add(lower.get(open..)?.find("</title")?);
        head.get(open..close).map(decode)
    });
    let pick = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| meta.get(*key).filter(|value| !value.is_empty()).cloned())
    };
    let title = pick(&["og:title", "twitter:title"]).or(title_tag)?;
    if title.is_empty() {
        return None;
    }
    let description = pick(&["og:description", "twitter:description", "description"]);
    let image = pick(&["og:image", "og:image:url", "twitter:image"])
        .and_then(|image| absolute(url, &image))
        .filter(|image| !image.chars().any(|c| c.is_whitespace() || c == '"'));
    Some(LinkPreview {
        url: url.to_owned(),
        title: clip(&title, MAX_TITLE),
        description: description
            .map(|text| clip(&text, MAX_DESCRIPTION))
            .filter(|text| !text.is_empty()),
        image,
        site: pick(&["og:site_name"]).map(|site| clip(&site, 80)),
    })
}

/// Fetches a page and reads its preview. Blocks.
fn fetch(http: &Http, url: &str) -> Option<LinkPreview> {
    let mut current = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        let response = http
            .send(Request {
                method: "GET".to_owned(),
                url: current.clone(),
                headers: vec![("accept".to_owned(), "text/html".to_owned())],
                body: Vec::new(),
                timeout: TIMEOUT,
            })
            .ok()?;
        let header = |name: &str| {
            response
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        };
        if (300..400).contains(&response.status) {
            current = absolute(&current, &header("location")?)?;
            continue;
        }
        let html = header("content-type").is_some_and(|kind| kind.contains("text/html"));
        if !(200..300).contains(&response.status) || !html {
            return None;
        }
        let mut preview = parse_html(&current, &response.body)?;
        // Show the link as people shared it.
        url.clone_into(&mut preview.url);
        return Some(preview);
    }
    None
}

/// Looks up a preview for the message's first link in the background and
/// shows it once found. Clears an old preview when the link is gone.
pub fn attach(state: &AppState, http: &Arc<Http>, message_id: i64, body: &str) {
    let link = first_link(body);
    let state = state.clone();
    let http = Arc::clone(http);
    tokio::spawn(async move {
        let result = async {
            let enabled = state.db.call(|conn| enabled(conn)).await?;
            let preview = match link {
                Some(link) if enabled => tokio::task::spawn_blocking(move || fetch(&http, &link))
                    .await
                    .ok()
                    .flatten(),
                _ => None,
            };
            let changed = state
                .db
                .call(move |conn| store::set_preview(conn, message_id, preview.as_ref()))
                .await?;
            if changed {
                messages::refresh(&state, message_id).await?;
            }
            AppResult::Ok(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(?error, "could not update a link preview");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_outside_code() {
        assert_eq!(
            first_link("See https://example.com/a?b=c, it's good.").as_deref(),
            Some("https://example.com/a?b=c")
        );
        assert_eq!(
            first_link("[docs](https://example.com/docs)").as_deref(),
            Some("https://example.com/docs")
        );
        assert_eq!(first_link("`https://example.com` in code"), None);
        assert_eq!(first_link("```\nhttps://example.com\n```"), None);
        assert_eq!(first_link("no links"), None);
    }

    #[test]
    fn reads_open_graph_and_title() {
        let html = r#"<html><head><title>Plain &amp; simple</title>
            <meta property="og:title" content="The &quot;Porch&quot; Guide">
            <meta name='description' content='How to build   a porch.'>
            <meta property=og:image content="/img/porch.png">
            <meta property="og:site_name" content="Porches">"#;
        let preview = parse_html("https://example.com/guide", html).unwrap();
        assert_eq!(preview.title, "The \"Porch\" Guide");
        assert_eq!(
            preview.description.as_deref(),
            Some("How to build a porch.")
        );
        assert_eq!(
            preview.image.as_deref(),
            Some("https://example.com/img/porch.png")
        );
        assert_eq!(preview.site.as_deref(), Some("Porches"));
        let plain = parse_html("https://example.com", "<title>Just a title</title>").unwrap();
        assert_eq!(plain.title, "Just a title");
        assert!(parse_html("https://example.com", "<p>nothing</p>").is_none());
    }
}
