//! Renders GitHub-flavored Markdown messages to safe HTML.
//!
//! People and automations write Markdown: emphasis, headings, lists, task
//! lists, tables, strikethrough, fenced code, block quotes and alerts such
//! as `> [!NOTE]`. A single line break is kept, as in chat. On top of that
//! come Sideporch's `:emoji:` shortcodes, `@mentions` and bare URLs from
//! [`markup::decorate`], and ```` ```mermaid ```` blocks, which the page
//! script draws as diagrams.
//!
//! Raw HTML is shown as text. Links need an `http`, `https` or `mailto`
//! URL or a path on this server. Images become links, so a message cannot
//! load pictures from elsewhere just by being read.

use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};

use crate::markup::{self, Context};

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
}

/// A link target that is safe to put in `href`.
fn safe_target(url: &str) -> bool {
    markup::is_safe_url(url)
        || (url.starts_with('/')
            && !url.starts_with("//")
            && !url
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '\\'))
}

fn open_link(url: &str) -> String {
    let mut out = String::from(r#"<a href=""#);
    markup::escape_text(&mut out, url);
    out.push_str(r#"" target="_blank" rel="noopener noreferrer nofollow">"#);
    out
}

/// Renders `text` to HTML that is safe to embed in a page.
pub fn render(text: &str, ctx: &Context) -> String {
    let mut events: Vec<Event<'_>> = Vec::new();
    // Whether each open link or image produced an `<a>` tag.
    let mut links: Vec<bool> = Vec::new();
    let mut in_code = false;
    let mut mermaid: Option<String> = None;
    for event in Parser::new_ext(text, options()) {
        if let Some(diagram) = &mut mermaid {
            match event {
                Event::Text(part) => diagram.push_str(&part),
                Event::End(TagEnd::CodeBlock) => {
                    let mut html = String::from(r#"<pre class="mermaid">"#);
                    markup::escape_text(&mut html, diagram.trim_end());
                    html.push_str("</pre>\n");
                    events.push(Event::Html(CowStr::from(html)));
                    mermaid = None;
                }
                _ => {}
            }
            continue;
        }
        let mapped = match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(language)))
                if language.trim().eq_ignore_ascii_case("mermaid") =>
            {
                mermaid = Some(String::new());
                continue;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code = true;
                Event::Start(Tag::CodeBlock(kind))
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                Event::End(TagEnd::CodeBlock)
            }
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                let safe = safe_target(&dest_url);
                links.push(safe);
                Event::Html(CowStr::from(if safe {
                    open_link(&dest_url)
                } else {
                    String::new()
                }))
            }
            Event::End(TagEnd::Link | TagEnd::Image) => {
                let safe = links.pop().unwrap_or(false);
                Event::Html(CowStr::from(if safe { "</a>" } else { "" }))
            }
            // Text inside code blocks and links is escaped by the writer.
            Event::Text(part) if !in_code && links.is_empty() => {
                let mut html = String::with_capacity(part.len());
                markup::decorate(&mut html, &part, ctx);
                Event::InlineHtml(CowStr::from(html))
            }
            Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
            Event::SoftBreak => Event::HardBreak,
            other => other,
        };
        events.push(mapped);
    }
    let mut out = String::with_capacity(text.len().saturating_mul(2));
    html::push_html(&mut out, events.into_iter());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(text: &str) -> String {
        let mut ctx = Context::default();
        ctx.usernames.insert("ada".to_owned());
        render(text, &ctx)
    }

    #[test]
    fn renders_github_flavored_markdown() {
        assert_eq!(
            md("**bold** _it_ ~~gone~~"),
            "<p><strong>bold</strong> <em>it</em> <del>gone</del></p>\n"
        );
        let table = md("| a | b |\n| - | - |\n| 1 | 2 |");
        assert!(
            table.contains("<table>") && table.contains("<td>2</td>"),
            "{table}"
        );
        let tasks = md("- [x] done\n- [ ] open");
        assert!(
            tasks.contains(r#"<input disabled="" type="checkbox" checked=""/>"#),
            "{tasks}"
        );
        assert!(md("# Title").contains("<h1>Title</h1>"));
        assert!(md("> [!NOTE]\n> Heads up").contains("markdown-alert-note"));
    }

    #[test]
    fn keeps_line_breaks_like_chat() {
        assert_eq!(md("one\ntwo"), "<p>one<br />\ntwo</p>\n");
    }

    #[test]
    fn adds_emoji_mentions_and_bare_links_outside_code() {
        let html = md("hi @ada :tada: see https://example.com `:tada: @ada`");
        assert!(
            html.contains(r#"class="mention""#) || html.contains("@ada"),
            "{html}"
        );
        assert!(html.contains("🎉"), "{html}");
        assert!(html.contains(r#"<a href="https://example.com""#), "{html}");
        assert!(html.contains("<code>:tada: @ada</code>"), "{html}");
    }

    #[test]
    fn escapes_html_and_unsafe_links() {
        let html = md("<script>alert(1)</script> <b>x</b>\n\n<div onclick=x>y</div>");
        assert!(
            !html.contains("<script>") && !html.contains("<b>") && !html.contains("<div"),
            "{html}"
        );
        assert!(html.contains("&lt;script&gt;"), "{html}");
        let link = md("[click](javascript:alert(1)) [ok](https://example.com) [here](/c/1)");
        assert!(!link.contains("javascript:"), "{link}");
        assert!(link.contains(r#"href="https://example.com""#) && link.contains(r#"href="/c/1""#));
        let image = md("![logo](https://example.com/x.png)");
        assert!(!image.contains("<img"), "{image}");
        assert!(
            image.contains(r#"<a href="https://example.com/x.png""#),
            "{image}"
        );
    }

    #[test]
    fn marks_mermaid_blocks_for_the_page_script() {
        let html = md("```mermaid\ngraph TD\n  A-->B & C\n```\n```rust\nfn main() {}\n```");
        assert!(
            html.contains("<pre class=\"mermaid\">graph TD\n  A--&gt;B &amp; C</pre>"),
            "{html}"
        );
        assert!(html.contains(r#"<code class="language-rust">"#), "{html}");
    }
}
