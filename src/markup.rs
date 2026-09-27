//! Renders Slack-style message markup ("mrkdwn") to safe HTML.
//!
//! Supported: `*bold*`, `_italic_`, `~strike~`, `` `code` ``, fenced code
//! blocks, `>` quotes, `<url|label>` links, bare URLs, `:emoji:` shortcodes,
//! and Slack's `&amp;` `&lt;` `&gt;` escapes. Everything else is escaped.

/// Renders `text` to HTML that is safe to embed in a page.
pub fn render(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(16));
    let parts: Vec<&str> = text.split("```").collect();
    // An odd number of fences leaves the last one unclosed; render it as text.
    let closed = if parts.len().is_multiple_of(2) {
        parts.len().saturating_sub(1)
    } else {
        parts.len()
    };
    let several = parts.len() > 1;
    for (index, part) in parts.into_iter().enumerate() {
        if index >= closed {
            out.push_str("```");
            lines(&mut out, part);
        } else if index.is_multiple_of(2) {
            lines(
                &mut out,
                if several {
                    part.trim_matches('\n')
                } else {
                    part
                },
            );
        } else {
            out.push_str("<pre><code>");
            escape_text(&mut out, part.trim_matches('\n'));
            out.push_str("</code></pre>");
        }
    }
    out
}

fn lines(out: &mut String, text: &str) {
    let mut in_quote = false;
    let mut first = true;
    for line in text.split('\n') {
        let quoted = line.strip_prefix('>').or_else(|| line.strip_prefix("&gt;"));
        let content = quoted.map_or(line, |rest| rest.strip_prefix(' ').unwrap_or(rest));
        if quoted.is_some() != in_quote {
            out.push_str(if in_quote {
                "</blockquote>"
            } else {
                "<blockquote>"
            });
            in_quote = quoted.is_some();
            first = true;
        }
        if !first {
            out.push_str("<br>");
        }
        inline(out, content);
        first = false;
    }
    if in_quote {
        out.push_str("</blockquote>");
    }
}

fn inline(out: &mut String, text: &str) {
    let mut rest = text;
    let mut prev: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        let at_word_start = prev.is_none_or(|p| !p.is_alphanumeric());
        let token = match c {
            '<' => slack_link(out, rest),
            '`' => code_span(out, rest),
            '*' if at_word_start => emphasis(out, rest, '*', "strong"),
            '_' if at_word_start => emphasis(out, rest, '_', "em"),
            '~' if at_word_start => emphasis(out, rest, '~', "s"),
            ':' => emoji(out, rest),
            'h' if at_word_start => bare_url(out, rest),
            '&' => entity(out, rest),
            _ => None,
        };
        if let Some(after) = token {
            prev = rest
                .get(..rest.len().saturating_sub(after.len()))
                .and_then(|t| t.chars().last());
            rest = after;
            continue;
        }
        let (current, after) = rest.split_at_checked(c.len_utf8()).unwrap_or((rest, ""));
        escape_text(out, current);
        prev = Some(c);
        rest = after;
    }
}

/// `<https://example.com|label>`, `<!here>`, `<@U123>` and `<#C123|name>`.
fn slack_link<'a>(out: &mut String, rest: &'a str) -> Option<&'a str> {
    let (inner, after) = rest.strip_prefix('<')?.split_once('>')?;
    if inner.is_empty() || inner.contains('<') {
        return None;
    }
    let (target, label) = inner
        .split_once('|')
        .map_or((inner, None), |(target, label)| (target, Some(label)));
    if let Some(special) = target.strip_prefix('!') {
        let name = special.split('^').next().unwrap_or(special);
        mention(out, "@", label.unwrap_or(name));
    } else if let Some(user) = target.strip_prefix('@') {
        mention(out, "@", label.unwrap_or(user));
    } else if let Some(channel) = target.strip_prefix('#') {
        mention(out, "#", label.unwrap_or(channel));
    } else if is_safe_url(target) {
        link(out, target, label.unwrap_or(target));
    } else {
        out.push_str("&lt;");
        escape_text(out, inner);
        out.push_str("&gt;");
    }
    Some(after)
}

fn mention(out: &mut String, sigil: &str, name: &str) {
    out.push_str(r#"<span class="font-semibold">"#);
    if !name.starts_with(sigil) {
        out.push_str(sigil);
    }
    escape_text(out, name);
    out.push_str("</span>");
}

fn link(out: &mut String, url: &str, label: &str) {
    out.push_str(r#"<a href=""#);
    escape_text(out, url);
    out.push_str(r#"" target="_blank" rel="noopener noreferrer nofollow">"#);
    escape_text(out, label);
    out.push_str("</a>");
}

fn code_span<'a>(out: &mut String, rest: &'a str) -> Option<&'a str> {
    let (code, after) = rest.strip_prefix('`')?.split_once('`')?;
    if code.is_empty() {
        return None;
    }
    out.push_str("<code>");
    escape_text(out, code);
    out.push_str("</code>");
    Some(after)
}

fn emphasis<'a>(out: &mut String, rest: &'a str, marker: char, tag: &str) -> Option<&'a str> {
    let tail = rest.strip_prefix(marker)?;
    for (index, _) in tail.match_indices(marker) {
        let (content, closing) = tail.split_at_checked(index)?;
        let after = closing.strip_prefix(marker)?;
        let trimmed = content.trim();
        if trimmed.is_empty() || trimmed.len() != content.len() {
            continue;
        }
        if after.chars().next().is_some_and(char::is_alphanumeric) {
            continue;
        }
        out.push('<');
        out.push_str(tag);
        out.push('>');
        inline(out, content);
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
        return Some(after);
    }
    None
}

fn emoji<'a>(out: &mut String, rest: &'a str) -> Option<&'a str> {
    let (name, after) = rest.strip_prefix(':')?.split_once(':')?;
    let glyph = emoji_glyph(name)?;
    out.push_str(r#"<span role="img" aria-label=""#);
    out.push_str(name);
    out.push_str(r#"">"#);
    out.push_str(glyph);
    out.push_str("</span>");
    Some(after)
}

fn bare_url<'a>(out: &mut String, rest: &'a str) -> Option<&'a str> {
    if !(rest.starts_with("https://") || rest.starts_with("http://")) {
        return None;
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '<' || c == '>')
        .unwrap_or(rest.len());
    let (candidate, _) = rest.split_at_checked(end)?;
    let url = candidate.trim_end_matches([
        '.', ',', ';', ':', '!', '?', '\'', '"', ')', ']', '*', '_', '~',
    ]);
    if url.len() <= "https://".len() {
        return None;
    }
    let (_, after) = rest.split_at_checked(url.len())?;
    link(out, url, url);
    Some(after)
}

/// Slack's `&amp;`, `&lt;` and `&gt;` escapes are valid HTML already.
fn entity<'a>(out: &mut String, rest: &'a str) -> Option<&'a str> {
    ["&amp;", "&lt;", "&gt;"].into_iter().find_map(|entity| {
        let after = rest.strip_prefix(entity)?;
        out.push_str(entity);
        Some(after)
    })
}

fn is_safe_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Escapes HTML special characters. Slack's own `&amp;`, `&lt;` and `&gt;`
/// escapes are already valid HTML and pass through unchanged.
fn escape_text(out: &mut String, text: &str) {
    for (index, c) in text.char_indices() {
        match c {
            '&' => {
                let tail = text.get(index..).unwrap_or_default();
                let entity = ["&amp;", "&lt;", "&gt;"]
                    .iter()
                    .any(|e| tail.starts_with(e));
                out.push_str(if entity { "&" } else { "&amp;" });
            }
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
}

/// Shortcodes commonly sent by monitors, CI systems and people.
fn emoji_glyph(name: &str) -> Option<&'static str> {
    Some(match name {
        "white_check_mark" => "✅",
        "heavy_check_mark" => "✔️",
        "x" => "❌",
        "warning" => "⚠️",
        "rotating_light" => "🚨",
        "helmet_with_white_cross" => "⛑️",
        "fire" => "🔥",
        "boom" => "💥",
        "red_circle" => "🔴",
        "large_green_circle" => "🟢",
        "large_yellow_circle" => "🟡",
        "large_blue_circle" => "🔵",
        "information_source" => "ℹ️",
        "bell" => "🔔",
        "rocket" => "🚀",
        "tada" => "🎉",
        "bug" => "🐛",
        "construction" => "🚧",
        "hourglass" => "⌛",
        "lock" => "🔒",
        "+1" | "thumbsup" => "👍",
        "-1" | "thumbsdown" => "👎",
        "eyes" => "👀",
        "wave" => "👋",
        "pray" => "🙏",
        "clap" => "👏",
        "heart" => "❤️",
        "smile" => "😄",
        "joy" => "😂",
        "thinking_face" => "🤔",
        "sunglasses" => "😎",
        "coffee" => "☕",
        "sunny" => "☀️",
        "house" => "🏠",
        "robot_face" => "🤖",
        "ghost" => "👻",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn escapes_html() {
        assert_eq!(
            render(r#"<script>alert("x")</script> & 'y'"#),
            "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;y&#39;"
        );
    }

    #[test]
    fn keeps_slack_entities() {
        assert_eq!(render("a &lt;b&gt; &amp; c"), "a &lt;b&gt; &amp; c");
    }

    #[test]
    fn formats_emphasis_at_word_boundaries() {
        assert_eq!(
            render("*bold* _it_ ~gone~ snake_case_name 2*3*4"),
            "<strong>bold</strong> <em>it</em> <s>gone</s> snake_case_name 2*3*4"
        );
        assert_eq!(render("* not bold *"), "* not bold *");
    }

    #[test]
    fn renders_code_without_formatting_inside() {
        assert_eq!(
            render("run `a *b* <c>`"),
            "run <code>a *b* &lt;c&gt;</code>"
        );
        assert_eq!(
            render("before\n```\nfn x() {}\n<tag>\n```\nafter"),
            "before<pre><code>fn x() {}\n&lt;tag&gt;</code></pre>after"
        );
        assert_eq!(render("half ``` fence"), "half ``` fence");
    }

    #[test]
    fn links_only_safe_urls() {
        assert_eq!(
            render("<https://example.com/a?b=1&c=2|the docs>"),
            r#"<a href="https://example.com/a?b=1&amp;c=2" target="_blank" rel="noopener noreferrer nofollow">the docs</a>"#
        );
        assert_eq!(
            render("<javascript:alert(1)|click>"),
            "&lt;javascript:alert(1)|click&gt;"
        );
        assert_eq!(
            render(r#"<https://x.test/"onmouseover="a>"#),
            r#"<a href="https://x.test/&quot;onmouseover=&quot;a" target="_blank" rel="noopener noreferrer nofollow">https://x.test/&quot;onmouseover=&quot;a</a>"#
        );
    }

    #[test]
    fn autolinks_bare_urls_without_trailing_punctuation() {
        assert_eq!(
            render("see https://gatus.io."),
            r#"see <a href="https://gatus.io" target="_blank" rel="noopener noreferrer nofollow">https://gatus.io</a>."#
        );
    }

    #[test]
    fn renders_mentions_and_quotes() {
        assert_eq!(
            render("<!here> hi\n> quoted\n> more\nafter"),
            r#"<span class="font-semibold">@here</span> hi<blockquote>quoted<br>more</blockquote>after"#
        );
    }

    #[test]
    fn replaces_known_emoji_only() {
        assert_eq!(
            render(":x: - :unknown: 10:30"),
            r#"<span role="img" aria-label="x">❌</span> - :unknown: 10:30"#
        );
    }
}
