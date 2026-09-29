//! What reading a message aloud says: its words, without Markdown, code
//! blocks or bare links.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// The words of a Markdown message, as sentences a voice can read.
pub fn speakable(markdown: &str) -> String {
    let mut out = String::new();
    let mut in_code_block = false;
    let push_break = |out: &mut String| {
        let trimmed = out.trim_end();
        if !trimmed.is_empty() && !trimmed.ends_with(['.', '!', '?', ':', ';']) {
            out.truncate(trimmed.len());
            out.push('.');
        }
        out.push(' ');
    };
    for event in Parser::new_ext(
        markdown,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS,
    ) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                out.push_str(" A code block. ");
            }
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Text(text) if !in_code_block => out.push_str(&words(&text)),
            Event::Code(code) => out.push_str(&code),
            Event::SoftBreak | Event::HardBreak => out.push(' '),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableRow
                | TagEnd::BlockQuote(_),
            ) => push_break(&mut out),
            Event::End(TagEnd::TableCell) => out.push_str(", "),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Plain text with emoji codes and bare links spoken more kindly.
fn words(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let lower = word.to_lowercase();
            if lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("www.")
            {
                "a link".to_owned()
            } else if word.len() > 2
                && word.starts_with(':')
                && word.ends_with(':')
                && !word.contains(' ')
            {
                // `:tada:` reads as its name.
                word.trim_matches(':').replace('_', " ")
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::speakable;

    #[test]
    fn reads_the_words() {
        assert_eq!(
            speakable("**Deploy** is done :tada:"),
            "Deploy is done tada."
        );
        assert_eq!(
            speakable("# Notes\n- one\n- two\n\nSee https://example.com/x"),
            "Notes. one. two. See a link."
        );
        assert_eq!(
            speakable("Run `make`:\n```\nrm -rf /\n```"),
            "Run make: A code block."
        );
        assert_eq!(speakable(""), "");
    }
}
