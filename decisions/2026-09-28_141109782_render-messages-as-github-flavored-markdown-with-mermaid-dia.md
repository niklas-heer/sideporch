+++
schema_version = 1
id = "01M3M5NWTPH0DPX413WKB69D77"
title = "Render messages as GitHub-flavored Markdown with Mermaid diagrams"
date = "2026-09-28"
status = "accepted"
tags = ["frontend", "messages"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Messages from people and automations are GitHub-flavored Markdown, rendered on the server with [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark): tables, task lists, strikethrough, footnotes, headings and GitHub's `> [!NOTE]` alerts. A single line break stays a line break, as people expect in chat. Sideporch's `:emoji:` shortcodes, `@mentions` and bare URLs are applied to text outside code and links.

Messages from Slack-compatible webhooks keep Slack's mrkdwn renderer, because tools such as Gatus send `*bold*` and `<url|label>` and mean Slack's formatting.

Output stays safe: raw HTML is shown as text; links need an `http`, `https` or `mailto` URL or a path on the server; images are shown as links, so reading a message never loads a picture from elsewhere.

```` ```mermaid ```` blocks become diagrams in the browser. Sideporch embeds Mermaid 12.0.0 (`dist/mermaid.min.js`, MIT), gzip-compressed to 1.6 MB and served with `Content-Encoding: gzip`. The page script loads it only when a page shows a diagram and runs it with `securityLevel: "strict"`. The bundle uses no `eval`, so the Content Security Policy stays unchanged.

## Context

On 2026-09-28 Niklas asked to "go beyond Slack and support GitHub-flavored Markdown", and suggested Mermaid in messages.

- **comrak** passes the full GFM spec, including bare-URL autolinks, but pulldown-cmark's event stream makes it simple to escape raw HTML, rewrite links and images, and add Sideporch's shortcodes in one pass. Bare URLs are handled by the existing mrkdwn helper.
- **Rendering Mermaid on the server** would avoid shipping JavaScript, but there is no mature Rust implementation; the reference implementation is JavaScript.
- **Loading Mermaid from a CDN** would keep the binary small but break the self-contained, offline-capable design and add a third-party request.

## Consequences

- `*text*` in people's messages is now italic, not bold as in Slack. Messages written before this change render with the new rules.
- The binary grows by about 1.6 MB. Updating Mermaid follows `assets/vendor/README.md`.
