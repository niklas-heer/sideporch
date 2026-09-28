+++
schema_version = 1
id = "01M3M94K5ZR835B0F5JHKWHX4B"
title = "Offer every standard emoji from gemoji"
date = "2026-09-28"
status = "accepted"
tags = ["messages", "frontend"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch knows every standard emoji from GitHub's [gemoji](https://github.com/github/gemoji) (MIT), vendored as `assets/vendor/emoji.tsv` (1,870 emoji with names, categories, keywords and descriptions) and regenerated with `scripts/update-emoji.sh`.

- Any gemoji alias works as a `:shortcode:` in messages and reactions, next to a short list of Slack spellings (`thinking_face`, `thumbsup`, …) and the instance's custom emoji.
- The reaction picker renders only the person's emoji and the custom emoji with the page. The full catalog is a static, cached JSON asset that loads when the picker first opens, grouped into nine categories with tabs and searchable by name and keyword. Without JavaScript, the reaction page lists everything.
- People choose up to twelve favorite emoji in their profile; without favorites, the ones they used most come first, topped up with popular defaults.

## Context

On 2026-09-28 Niklas asked for all emoji in the reaction picker, sorted nicely, with each person's top emoji first.

- **Unicode's emoji-test.txt** has every emoji and category but no shortcode names or search keywords.
- **Rendering every emoji into each page** would add about 150 kB of markup per page.
- **emoji-mart's data** is richer but larger and aimed at its own JavaScript component.

## Consequences

- The binary carries about 90 kB of emoji data.
- New Unicode emoji arrive when gemoji adds them and the table is regenerated.
