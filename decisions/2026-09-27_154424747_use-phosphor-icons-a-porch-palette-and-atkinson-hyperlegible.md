+++
schema_version = 1
id = "01M3HRKXNBNXQ3MMQB6FEC9CPT"
title = "Use Phosphor icons, a porch palette, and Atkinson Hyperlegible Next"
date = "2026-09-27"
status = "accepted"
tags = ["frontend", "design"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3HRKXN442S40GZ9AGYGQHH1"]
+++
## Decision

Sideporch's visual identity:

- **Icons**: [Phosphor](https://phosphoricons.com) (MIT), regular weight, through the [`phosphor-svgs`](https://crates.io/crates/phosphor-svgs) crate. It exposes every icon as a `const &str` SVG, so the icons used are inlined into the HTML and inherit the text colour. `src/icons.rs` is the single place that imports them.
- **Logo**: `assets/logo.svg`, a lean-to "side porch" roof and post in porch-ceiling blue sheltering a lamp-yellow speech bubble, on painted-floor green. It is also the favicon and the web-app icon.
- **Palette** (`encre-css.toml`): floor green `#24403C` for the sidebar and buttons, haint blue `#B9E0DA` for highlights, and porch-light yellow `#F5C04A` used only for unread markers.
- **Typeface**: [Atkinson Hyperlegible Next](https://github.com/googlefonts/atkinson-hyperlegible-next) (SIL OFL 1.1), embedded as four woff2 subsets (about 110 kB) in `assets/fonts/`.

## Context

On 2026-09-27 Niklas asked for "a nice SVG icon" library and a project icon, and left the choice to the agent. Rust crates were compared on crates.io that day:

- `lucide-icons` (current, widely used) ships Lucide as an icon font, not SVG.
- `icondata_lu` has SVG data for Lucide, but had not been updated since June 2025, and is shaped for Leptos.
- `heroicons` and `lucide-svg` are small, rarely updated wrappers.
- `phosphor-svgs` 0.5.0 was regenerated from upstream on 2026-09-24, has no required dependencies, and its constants drop straight into maud with `PreEscaped`.

The palette comes from the name: Southern porch ceilings are traditionally painted "haint blue", floors a deep green, and the porch light marks that someone is around. It avoids the cream-and-terracotta look common to generated interfaces. Atkinson Hyperlegible was designed for readers with low vision, which suits a chat meant for families and clubs as well as teams.

## Consequences

- `phosphor-svgs` has few downloads. If it stops tracking upstream, the SVGs can be vendored from Phosphor's repository without changing call sites in `src/icons.rs`.
- The font adds about 110 kB to the binary. Fonts are served with long cache lifetimes.
- The font's OFL licence text ships in `assets/fonts/OFL.txt` and must stay with the files.
