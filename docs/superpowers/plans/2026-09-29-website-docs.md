# sideporch.app with documentation: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `website/` into a Zola site with a polished landing page, searchable documentation that replaces most of the README, zoomable screenshots, and screenshots retaken from the current code.

**Architecture:** Zola renders Markdown under `website/content/` through our own templates; Pagefind indexes the built docs. Both are pinned in `mise.toml`/`mise.lock` and installed the same way locally, in Dagger, and on Vercel. One small script (`static/site.js`) adds the hero animation, copy buttons, search, the docs drawer and the image viewer; everything works without it except search.

**Tech Stack:** Zola 0.23.6, Pagefind 1.5.2, plain CSS, vanilla JS, Rust test for the generated API page, Playwright (existing `scripts/screenshots.mjs`), cwebp, Dagger (Dang), Vercel.

**Spec:** `docs/superpowers/specs/2026-09-29-website-docs-design.md`

## Global Constraints

- Zola `0.23.6` (mise registry `zola`), Pagefind `1.5.2` (`github:CloudCannon/pagefind`); both locked in `mise.lock` for linux-x64, linux-arm64, macos-arm64, macos-x64.
- No Node toolchain in the site build. No Sass. No CSS framework.
- Palette and font: reuse the variables in today's `website/style.css` (`--floor`, `--haint`, `--lamp`, …) and Atkinson Hyperlegible Next from `website/fonts/`.
- Light and dark follow `prefers-color-scheme`.
- The site works without JavaScript except search.
- Copy style: plain, second person, British/American mix as in the README ("colour" only where existing text uses it), no marketing superlatives, sentence-case headings.
- Pages describe `main`. Anything newer than the latest release (`Cargo.toml` version, currently 0.4.0) says "New in X.Y" with the next version.
- One copy of each screenshot, in `website/static/img/`.
- Every claim in the docs is checked against the code before it is written (see Task 6's checklist).
- Commits: conventional (`docs(website): …`, `ci: …`, `test: …`), each ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Commit on `main` (the repo's workflow).
- Clean up afterwards: `mise run clean-debug`, remove `website/public/`, temp data directories and `/tmp/shots`.

## Review Focus

1. **Deep links from outside keep working.** People have `https://sideporch.app/#install` and `#features` bookmarked, and GitHub links to `README.md#install` etc. Expected: the landing page keeps `id="features"` and `id="install"`; the slim README keeps `## Install` and `## Try it` headings (short, linking to the docs), so old anchors land somewhere sensible. Pinned in Task 11 Step 5 and Task 12 Step 2.
2. **The image viewer never traps people.** Opening an image, then pressing Back, Esc, the ✕, or clicking outside must close it and leave the page where it was, including after stepping through several images with ←/→ (Back must not walk through each image). Pinned in Task 4's browser checklist.
3. **Build tools missing or a different version.** Someone runs `mise run site` without having installed the tools, or Vercel's image changes. Expected: tasks install from `mise.lock`, and the Vercel script fails loudly on checksum mismatch instead of building with whatever is on PATH. Pinned in Task 1 Step 6.
4. **Docs drift from the code.** A new `sideporch.*` function or event, a renamed CLI flag. Expected: the API page test fails `mise run check`; CLI options are copied from `src/main.rs` and checked in Task 6. Pinned in Task 5.
5. **Narrow phones and long content.** 320–375 px wide screens with long code lines (`curl … | sh`), wide tables (search filters, options) and the docs sidebar. Expected: code and tables scroll horizontally inside their box; the page itself never scrolls sideways; the sidebar becomes a drawer. Pinned in Task 2's browser checklist and Task 14.

---

## File map

```
mise.toml                         + zola, pagefind; tasks site, site-build, site-check, screenshots
mise.lock                         + zola, pagefind entries
.dagger/main.dang                 + website in source; site function; ci calls it
flake.nix                         + generated API page in the package's fileset
vercel.json → website/vercel.json   (stays in website/, gains buildCommand, outputDirectory)
website/
  config.toml                     Zola config
  vercel-build.sh                 installs pinned mise, then tools, then builds (Vercel only)
  README.md                       how the site works
  content/_index.md               landing (front matter only; body in template)
  content/docs/_index.md          docs home
  content/docs/get-started/…      5 pages
  content/docs/using/…            8 pages
  content/docs/community/…        9 pages
  content/docs/integrations/…     5 pages (automation-api.md generated)
  templates/base.html             head, nav, footer, search dialog, image viewer dialog
  templates/index.html            landing page
  templates/docs.html             a docs page
  templates/docs-section.html     a docs section (and docs home)
  templates/macros/docs.html      sidebar, prev/next
  templates/shortcodes/note.html  callout
  templates/shortcodes/shot.html  zoomable screenshot
  templates/404.html
  static/style.css                base, landing, docs, viewer, search
  static/site.js                  (was porch.js) animation, copy, search, drawer, viewer
  static/fonts/, static/img/, static/logo.svg, static/og.png
src/automations/api.rs            + test that keeps the docs API page current
scripts/screenshots.mjs           + new shots; header points to mise task
scripts/screenshots.sh            new: the whole retake run
README.md                         slimmed
AGENTS.md                         docs location and rules
tools/loadtest/report.sh          points at the new capacity page
docs/capacity.md, docs/screenshots/   removed
decisions/<new>.md                vrdx record
```

---

### Task 1: Zola scaffold, pinned tools, and the landing page ported unchanged

Moves today's page into Zola without changing how it looks, so later diffs show only intended changes.

**Files:**
- Modify: `mise.toml`, `mise.lock`, `.dagger/main.dang`
- Create: `website/config.toml`, `website/content/_index.md`, `website/templates/base.html`, `website/templates/index.html`, `website/vercel-build.sh`
- Move: `website/{style.css,logo.svg,fonts/,img/}` → `website/static/…`; `website/porch.js` → `website/static/site.js`; `website/index.html` → content split into the two templates
- Modify: `website/vercel.json`, `website/README.md`, `.gitignore` (add `website/public/`)

**Interfaces:**
- Produces: `templates/base.html` with blocks `title`, `description`, `canonical`, `head`, `body_class`, `header`, `content`; global nav partial inside it. Tasks 2–4 and 11 extend it.
- Produces: mise tasks `site`, `site-build`, `site-check`.
- Produces: Dagger function `site: Directory!` (the built `public/`).

- [ ] **Step 1: Pin the tools**

Add to `[tools]` in `mise.toml` after `git-cliff`:

```toml
# The website (website/): a static site generator and its search index.
zola = "0.23.6"
"github:CloudCannon/pagefind" = "1.5.2"
```

Run: `rtk mise lock --platform linux-x64,linux-arm64,macos-arm64,macos-x64 && rtk mise install zola github:CloudCannon/pagefind && zola --version && pagefind --version`
Expected: `zola 0.23.6`, `pagefind 1.5.2`; `mise.lock` gains `[[tools.zola]]` and `[[tools."github:CloudCannon/pagefind"]]` with four platforms each. If the pagefind GitHub release names assets such that mise picks the `pagefind_extended` binary, add `{ version = "1.5.2", asset_pattern = "pagefind-v1.5.2-*" }` (check `mise ls-remote` and the release page).

- [ ] **Step 2: Add the tasks**

Append to `mise.toml`:

```toml
[tasks.site]
description = "Preview the website with live reload at http://127.0.0.1:1111"
dir = "website"
run = "zola serve"

[tasks."site-build"]
description = "Build the website into website/public, with its search index"
dir = "website"
run = ["zola build", "pagefind --site public"]

[tasks."site-check"]
description = "Build the website and check every internal and external link"
dir = "website"
run = ["zola check", "mise run site-build"]
```

- [ ] **Step 3: Write `website/config.toml`**

Check the 0.23 configuration names first (`rtk zola init /tmp/zola-probe` and read the generated file, or Context7 `getzola/zola` docs for "highlighting"), then write:

```toml
base_url = "https://sideporch.app"
title = "Sideporch"
description = "Your own chat for a team, a club or a community. One program, on a server you control."
default_language = "en"
compile_sass = false
build_search_index = false   # Pagefind builds the search index
generate_feeds = false

[markdown]
# Code highlighting with a light and a dark theme, as CSS classes so
# style.css switches them with prefers-color-scheme. Use the 0.23 key names.
smart_punctuation = true
bottom_footnotes = true

[link_checker]
internal_level = "error"
external_level = "warn"
skip_prefixes = ["http://localhost", "http://127.0.0.1", "https://chat.example.com"]

[extra]
repo = "https://github.com/niklas-heer/sideporch"
edit_base = "https://github.com/niklas-heer/sideporch/edit/main/website/content/"
```

Fill in the highlighting keys found in the probe so that code is highlighted with CSS classes (Zola writes the theme CSS files; reference both from `base.html` with `media="(prefers-color-scheme: …)"`). Delete `/tmp/zola-probe`.

- [ ] **Step 4: Split `index.html` into `base.html` and `index.html`**

`templates/base.html`: everything from `<!doctype html>` to `<body>` (with `{% block title %}`, `{% block description %}`, `{% block canonical %}{{ current_url | safe }}{% endblock %}`, `{% block head %}{% endblock %}`), the `<body class="{% block body_class %}{% endblock %}">`, `{% block header %}{% endblock %}`, `{% block content %}{% endblock %}`, and the footer. Asset URLs use `{{ get_url(path="style.css") }}` etc. The `<script>` becomes `site.js`.

`templates/index.html`: `{% extends "base.html" %}`, the dusk block (nav + hero) in `header`, `<main>` in `content`. `content/_index.md`:

```markdown
+++
title = "Sideporch: your own chat for a team, a club or a community"
template = "index.html"
+++
```

Move the static files with `git mv` so history follows them.

- [ ] **Step 5: Verify the port is visually identical**

Run: `rtk mise run site-build && python3 -m http.server -d website/public 8765` (background), open `http://127.0.0.1:8765/` in the preview browser at 1440 and 375 px, compare with `git stash`-free reference: `python3 -m http.server -d <(git show HEAD:website)`—simpler: before Step 4, take `preview_screenshot`s of the old page served from a `git worktree` of `HEAD` at the same widths; after, take them again and compare by eye.
Expected: same page; hero animation plays; Copy works.

- [ ] **Step 6: Vercel build script, with pinned mise**

`website/vercel-build.sh`:

```sh
#!/bin/sh
# Vercel's build: install the pinned mise, then the site tools from
# mise.lock (checksums verified), then build into website/public.
set -eu
mise_version=2026.9.5
case "$(uname -m)" in
  x86_64) platform=linux-x64; sha256=d71e94e1ed59d4d0ca4ac847fa321d6d6615a8e613e9b468c9fb39f0dddd06d5 ;;
  aarch64) platform=linux-arm64; sha256=3a52c7c7c58d21a0791516950ebf4bc915f403277b49c93d657fc585259625ec ;;
  *) echo "no mise for $(uname -m)" >&2; exit 1 ;;
esac
archive="mise-v$mise_version-$platform.tar.gz"
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
curl -fsSL "https://github.com/jdx/mise/releases/download/v$mise_version/$archive" -o "$tmp/$archive"
echo "$sha256  $tmp/$archive" | sha256sum -c -
tar -xzf "$tmp/$archive" -C "$tmp"
export PATH="$tmp/mise/bin:$PATH" MISE_YES=1 MISE_TRUSTED_CONFIG_PATHS="$PWD"
mise install zola github:CloudCannon/pagefind
mise run site-build
```

The checksums come from `https://github.com/jdx/mise/releases/download/v2026.9.5/SHASUMS256.txt`; keep `mise_version` in step with `min_version` in `mise.toml` and the Dagger image. `website/vercel.json`:

```json
{
  "$schema": "https://openapi.vercel.sh/vercel.json",
  "cleanUrls": true,
  "trailingSlash": true,
  "buildCommand": "sh vercel-build.sh",
  "outputDirectory": "public",
  "ignoreCommand": "[ -n \"$VERCEL_GIT_PREVIOUS_SHA\" ] && git diff --quiet \"$VERCEL_GIT_PREVIOUS_SHA\" \"$VERCEL_GIT_COMMIT_SHA\" -- . ../mise.toml ../mise.lock",
  "headers": [
    { "source": "/(fonts|img|pagefind)/(.*)", "headers": [{ "key": "Cache-Control", "value": "public, max-age=86400" }] }
  ]
}
```

Test the script's failure path: temporarily change one checksum character and run `sh website/vercel-build.sh` in a Linux container (`container run --rm -v "$PWD":/src -w /src debian:stable-slim sh -c 'apt-get update -qq && apt-get install -qq -y curl ca-certificates >/dev/null && sh website/vercel-build.sh'`, or Colima `docker run` equivalently).
Expected: `sha256sum: WARNING: 1 computed checksum did NOT match`, non-zero exit. Restore the checksum and rerun: builds `website/public/index.html`.

Note for Niklas in the final report: Vercel's project settings must allow the build (Framework preset "Other", root `website`, no overriding build command in the dashboard).

- [ ] **Step 7: CI builds the site**

In `.dagger/main.dang` add `"!website"` to the source ignore patterns, add `zola` and `github:CloudCannon/pagefind` to the `mise install` in `toolchain`, and add:

```
  """
  Build the website and check its internal links. Returns the built site.
  """
  pub site: Directory! {
    toolchain
      .withExec(["mise", "exec", "--", "sh", "-c", "cd website && zola check --skip-external-links && zola build && pagefind --site public"])
      .directory("/src/website/public")
  }
```

and make `ci` depend on it: `pub ci: String! { site.entries; toolchain.withExec(["mise", "run", "check"]).stdout }` (use whatever sequencing Dang supports — check `.dagger` docs / `dagger functions`; if statements aren't sequenced, return `toolchain.withDirectory("/site", site).withExec(["mise", "run", "check"]).stdout`). (`zola check --skip-external-links` exists in 0.23.6.)

Run: `rtk dagger call site export --path /tmp/site-out && ls /tmp/site-out`
Expected: `index.html`, `pagefind/`. Remove `/tmp/site-out`.

- [ ] **Step 8: README for the site, then commit**

Rewrite `website/README.md`: Zola + Pagefind, `mise run site` / `site-build` / `site-check`, the layout above, `vercel-build.sh`, and that screenshots come from `mise run screenshots` (Task 10).

```bash
rtk git add -A website mise.toml mise.lock .dagger/main.dang .gitignore
rtk git commit -m "docs(website): build sideporch.app with Zola and Pagefind"
```

---

### Task 2: Docs layout: sidebar, contents, prev/next, callouts, 404

**Files:**
- Create: `website/content/docs/_index.md`, `website/content/docs/{get-started,using,community,integrations}/_index.md`, `website/templates/{docs.html,docs-section.html,404.html}`, `website/templates/macros/docs.html`, `website/templates/shortcodes/note.html`
- Modify: `website/static/style.css`, `website/static/site.js`, `website/templates/base.html`

**Interfaces:**
- Consumes: `base.html` blocks from Task 1.
- Produces: section front matter convention `sort_by = "weight"`, `weight = N`, `template = "docs-section.html"`, `page_template = "docs.html"`; page front matter `title`, `description`, `weight`. Tasks 6–9 write pages in this shape.
- Produces: shortcode `{% note(kind="tip"|"warning"|"info") %}…{% end %}`.

- [ ] **Step 1: Section files**

`content/docs/_index.md`:

```markdown
+++
title = "Documentation"
description = "Install, run and use Sideporch."
sort_by = "weight"
template = "docs-section.html"
page_template = "docs.html"
+++
```

Each of the four sections (`get-started` weight 1 "Get started", `using` 2 "Using Sideporch", `community` 3 "Run a community", `integrations` 4 "Integrations"):

```markdown
+++
title = "Get started"
description = "Try Sideporch, install it on a server and keep it up to date."
weight = 1
sort_by = "weight"
template = "docs-section.html"
page_template = "docs.html"
+++
```

- [ ] **Step 2: Sidebar and prev/next macros**

`templates/macros/docs.html`:

```jinja
{% macro sidebar(current) %}
{% set docs = get_section(path="docs/_index.md") %}
<nav class="docs-nav" id="docs-nav" aria-label="Documentation">
  <a class="docs-home{% if current == docs.permalink %} here{% endif %}" href="{{ docs.permalink }}">Overview</a>
  {% for path in docs.subsections %}
    {% set section = get_section(path=path) %}
    <h2>{{ section.title }}</h2>
    <ul>
      {% for p in section.pages %}
        <li><a href="{{ p.permalink }}"{% if current == p.permalink %} aria-current="page"{% endif %}>{{ p.title }}</a></li>
      {% endfor %}
    </ul>
  {% endfor %}
</nav>
{% endmacro %}

{% macro neighbours(current) %}
{# Across sections: flatten the docs in sidebar order. #}
{% set docs = get_section(path="docs/_index.md") %}
{% set_global flat = [] %}
{% for path in docs.subsections %}
  {% for p in get_section(path=path).pages %}{% set_global flat = flat | concat(with=p) %}{% endfor %}
{% endfor %}
{% for p in flat %}{% if p.permalink == current %}
<nav class="pager" aria-label="Previous and next">
  {% if not loop.first %}{% set prev = flat[loop.index0 - 1] %}<a rel="prev" href="{{ prev.permalink }}"><span>Previous</span>{{ prev.title }}</a>{% endif %}
  {% if not loop.last %}{% set next = flat[loop.index0 + 1] %}<a rel="next" href="{{ next.permalink }}"><span>Next</span>{{ next.title }}</a>{% endif %}
</nav>
{% endif %}{% endfor %}
{% endmacro %}
```

(`docs.subsections` is ordered by the subsections' weight in 0.23; if not, sort with `| sort(attribute="weight")` over the fetched sections.)

- [ ] **Step 3: `docs.html` and `docs-section.html`**

`docs.html`:

```jinja
{% extends "base.html" %}
{% import "macros/docs.html" as docs %}
{% block title %}{{ page.title }} · Sideporch docs{% endblock %}
{% block description %}{{ page.description | default(value=config.description) }}{% endblock %}
{% block body_class %}docs{% endblock %}
{% block content %}
<div class="docs-shell">
  <button class="docs-menu" type="button" aria-controls="docs-nav" aria-expanded="false" hidden>Menu</button>
  {{ docs::sidebar(current=page.permalink) }}
  <article class="prose" data-pagefind-body>
    <p class="crumb" data-pagefind-ignore>{{ get_section(path=page.ancestors | last).title }}</p>
    <h1>{{ page.title }}</h1>
    {{ page.content | safe }}
    <footer class="page-foot" data-pagefind-ignore>
      <a href="{{ config.extra.edit_base }}{{ page.relative_path }}">Edit this page on GitHub</a>
      {{ docs::neighbours(current=page.permalink) }}
    </footer>
  </article>
  {% if page.toc | length > 0 %}
  <nav class="toc" aria-label="On this page">
    <h2>On this page</h2>
    <ul>{% for h in page.toc %}<li><a href="{{ h.permalink | safe }}">{{ h.title }}</a>
      {% if h.children %}<ul>{% for c in h.children %}<li><a href="{{ c.permalink | safe }}">{{ c.title }}</a></li>{% endfor %}</ul>{% endif %}</li>{% endfor %}</ul>
  </nav>
  {% endif %}
</div>
{% endblock %}
```

Set `[markdown] insert_anchor_links = "heading"` (or the 0.23 equivalent) in `config.toml` so headings link to themselves.

`docs-section.html`: same shell; the article shows `section.content` and a card grid: for the docs home, one card per subsection (title, description, its pages as links); for a subsection, a list of its pages with descriptions.

- [ ] **Step 4: Callout shortcode**

`templates/shortcodes/note.html`:

```jinja
<aside class="note note-{{ kind | default(value='info') }}" role="note">
  <p class="note-label">{% if kind == "warning" %}Careful{% elif kind == "tip" %}Tip{% else %}Note{% endif %}</p>
  {{ body | markdown | safe }}
</aside>
```

- [ ] **Step 5: 404 page**

`templates/404.html` extends `base.html`: heading "This page isn't on the porch", a line of text, links to the home page and `/docs/`, and a search button (Task 3 wires it).

- [ ] **Step 6: Styles**

Add to `static/style.css` (after the landing styles, headed `/* Docs */`):
- `.docs-shell`: grid `16rem minmax(0, 1fr) 13rem`, max-width 80rem, gap 3rem; below 72rem drop the TOC column; below 52rem one column.
- `.docs-nav`: sticky top 1rem, `max-height: calc(100vh - 2rem)`, own scroll; `h2` small caps-ish label style (0.8rem, 800, muted, uppercase letter-spacing .06em); links 0.95rem; `[aria-current=page]` with `--haint-2` background (dark: `--floor-2`) and bold.
- Below 52rem: `.docs-menu` shown (JS removes `hidden`), `.docs-nav` is `position: fixed; inset: 0 auto 0 0; width: min(20rem, 85vw); transform: translateX(-100%)`; `.docs-nav.open` translates to 0 with a backdrop `::after` on `body.drawer-open`. Without JS the nav renders inline above the article (no `hidden` removal → keep nav static when `.js` class absent on `<html>`).
- `.prose`: `max-width: 46rem`; headings with `scroll-margin-top: 1.5rem`; `pre` with `overflow-x: auto`, radius .6rem, border; tables wrapped in `overflow-x: auto` (JS wraps `table` in `.table-scroll`; without JS, `display: block; overflow-x: auto` on `.prose table`); `code` inline style from the landing page.
- `.note`: left border 4px `--floor-3` (tip), `--lamp` (warning); padded, soft background.
- `.toc`: sticky, 0.9rem, muted; current heading bold (set by JS via IntersectionObserver).
- `.pager`: two cards side by side, "Previous"/"Next" small label.
- `.crumb`: small muted label above the title.
- The docs pages reuse the landing page's `.bar` nav on a solid `--floor` background (not the dusk hero).

- [ ] **Step 7: Script behaviour for docs**

In `static/site.js`, add (each block guarded by a `querySelector` check, as the existing code does):

```js
  document.documentElement.classList.add("js");

  // Docs drawer on narrow screens.
  const menu = document.querySelector(".docs-menu");
  const nav = document.getElementById("docs-nav");
  if (menu && nav) {
    menu.hidden = false;
    const set = (open) => {
      nav.classList.toggle("open", open);
      document.body.classList.toggle("drawer-open", open);
      menu.setAttribute("aria-expanded", String(open));
      if (open) nav.querySelector("a")?.focus();
    };
    menu.addEventListener("click", () => set(!nav.classList.contains("open")));
    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && nav.classList.contains("open")) { set(false); menu.focus(); }
    });
    document.addEventListener("click", (event) => {
      if (nav.classList.contains("open") && !nav.contains(event.target) && event.target !== menu) set(false);
    });
  }

  // Copy buttons on code blocks.
  for (const pre of document.querySelectorAll(".prose pre")) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "copy-code";
    button.textContent = "Copy";
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(pre.querySelector("code")?.innerText ?? pre.innerText);
        button.textContent = "Copied";
      } catch {
        button.textContent = "Select it";
      }
      setTimeout(() => (button.textContent = "Copy"), 2000);
    });
    pre.append(button);
  }

  // Wide tables scroll inside their own box.
  for (const table of document.querySelectorAll(".prose table")) {
    const box = document.createElement("div");
    box.className = "table-scroll";
    table.replaceWith(box);
    box.append(table);
  }

  // Mark the heading you're reading in "On this page".
  const toc = document.querySelector(".toc");
  if (toc && "IntersectionObserver" in window) {
    const links = new Map([...toc.querySelectorAll("a")].map((a) => [decodeURIComponent(a.hash.slice(1)), a]));
    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        for (const a of links.values()) a.removeAttribute("aria-current");
        links.get(entry.target.id)?.setAttribute("aria-current", "true");
      }
    }, { rootMargin: "0px 0px -70% 0px" });
    for (const id of links.keys()) {
      const heading = document.getElementById(id);
      if (heading) observer.observe(heading);
    }
  }
```

- [ ] **Step 8: Check in the browser**

Add a throwaway `content/docs/get-started/probe.md` with a long code line, a wide 6-column table, three `##` headings with `###` children, a `note` of each kind. `rtk mise run site-build`, serve `website/public`, check at 1440, 900, 375 and 320 px, light and dark (`preview_set_appearance`):
- sidebar marks the page; TOC shows and highlights while scrolling (1440 only);
- at 375/320 the page never scrolls sideways (`preview_evaluate`: `document.documentElement.scrollWidth <= innerWidth`), the Menu button opens the drawer, Esc and a click outside close it, focus returns to Menu;
- copy button copies the code;
- prev/next go to the neighbouring pages; 404 page renders at `/nope/`.
Delete `probe.md`.

- [ ] **Step 9: Commit**

```bash
rtk git add -A website
rtk git commit -m "docs(website): documentation layout with sidebar, contents and callouts"
```

---

### Task 3: Search with Pagefind

**Files:**
- Modify: `website/templates/base.html`, `website/static/site.js`, `website/static/style.css`, `website/templates/index.html` (landing excluded from index)

**Interfaces:**
- Consumes: `data-pagefind-body` on docs articles (Task 2).
- Produces: any element with `data-search-open` opens search. Task 11 puts one in the landing nav; the 404 page's button gets the attribute.

- [ ] **Step 1: Dialog markup in `base.html`**

Before `</body>`:

```html
<dialog class="search" id="search" aria-label="Search the documentation">
  <form method="dialog" class="search-bar">
    <input type="search" id="search-input" placeholder="Search the docs" autocomplete="off" aria-controls="search-results">
    <button type="submit" aria-label="Close search">Esc</button>
  </form>
  <ol id="search-results" class="search-results" aria-live="polite"></ol>
</dialog>
```

and in the nav: `<li><button type="button" class="nav-search" data-search-open aria-keyshortcuts="/ Control+K Meta+K">Search <kbd>/</kbd></button></li>`, styled as a link-like pill. No-JS: the button is `hidden` until `site.js` unhides it (search needs JS).

- [ ] **Step 2: Lazy Pagefind in `site.js`**

```js
  // Search: Pagefind's index loads the first time search opens.
  const search = document.getElementById("search");
  if (search) {
    const input = document.getElementById("search-input");
    const results = document.getElementById("search-results");
    let pagefind;
    let run = 0;
    const open = async () => {
      if (!search.open) search.showModal();
      input.focus();
      pagefind ??= await import("/pagefind/pagefind.js");
    };
    for (const button of document.querySelectorAll("[data-search-open]")) {
      button.hidden = false;
      button.addEventListener("click", open);
    }
    document.addEventListener("keydown", (event) => {
      const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName ?? "") || document.activeElement?.isContentEditable;
      if ((event.key === "/" && !typing) || (event.key.toLowerCase() === "k" && (event.metaKey || event.ctrlKey))) {
        event.preventDefault();
        open();
      }
    });
    search.addEventListener("click", (event) => { if (event.target === search) search.close(); });
    input.addEventListener("input", async () => {
      const mine = ++run;
      const query = input.value.trim();
      if (!query) { results.replaceChildren(); return; }
      pagefind ??= await import("/pagefind/pagefind.js");
      const found = await pagefind.debouncedSearch(query, {}, 150);
      if (!found || mine !== run) return;
      const items = await Promise.all(found.results.slice(0, 8).map((r) => r.data()));
      if (mine !== run) return;
      results.replaceChildren(...(items.length ? items.map((item) => {
        const li = document.createElement("li");
        const a = document.createElement("a");
        a.href = item.url;
        const title = document.createElement("strong");
        title.textContent = item.meta.title;
        const excerpt = document.createElement("span");
        excerpt.innerHTML = item.excerpt; // Pagefind escapes text and adds only <mark>
        a.append(title, excerpt);
        li.append(a);
        return li;
      }) : [Object.assign(document.createElement("li"), { className: "empty", textContent: `Nothing found for “${query}”.` })]));
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        results.querySelector("a")?.click();
      }
    });
  }
```

Arrow-key movement through results: `ArrowDown`/`ArrowUp` on the input and on result links move focus between `input` and `results` links.

- [ ] **Step 3: Exclude the landing page**

The landing template has no `data-pagefind-body`; because docs pages do, Pagefind indexes only those. Verify in Step 5.

- [ ] **Step 4: Styles**

`.search` dialog: top-aligned (`margin-top: 10vh`), `width: min(40rem, 92vw)`, rounded, `::backdrop` `rgb(16 26 25 / 0.55)`; input 1.1rem, full width; results with bold title and muted excerpt; `mark` in `--lamp` at 45% alpha; `a:focus-visible` outlined.

- [ ] **Step 5: Check**

Add two throwaway pages again (or use Task 6+ pages if already written). `rtk mise run site-build`, serve, then:
- `/` opens search; typing "backup" lists docs pages only (landing absent: `preview_evaluate` `await (await import('/pagefind/pagefind.js')).search('Everything people expect')` returns no landing result);
- Enter opens the first result; Esc and a backdrop click close; ⌘K works while focus is in the page but `/` doesn't trigger inside the input;
- the network panel shows `/pagefind/` requests only after opening search (`preview_evaluate` `performance.getEntriesByType('resource').some(e => e.name.includes('pagefind'))` is false before opening).

- [ ] **Step 6: Commit**

```bash
rtk git add -A website
rtk git commit -m "docs(website): search the documentation with Pagefind"
```

---

### Task 4: Zoomable images

**Files:**
- Create: `website/templates/shortcodes/shot.html`
- Modify: `website/templates/base.html`, `website/static/site.js`, `website/static/style.css`

**Interfaces:**
- Produces: markup contract for zoomable images, used by the landing gallery (Task 11) and docs (Tasks 6–9):

```html
<figure class="shot">
  <a href="/img/NAME.webp" data-zoom [data-zoom-dark="/img/NAME-dark.webp"]>
    <picture>…<img src="/img/NAME.webp" alt="…" width="W" height="H" loading="lazy"></picture>
  </a>
  <figcaption>…</figcaption>
</figure>
```

- Produces: shortcode `{{ shot(name="search", alt="…", caption="…", dark=false) }}` rendering the markup above, reading width/height with `get_image_metadata(path="static/img/" ~ name ~ ".webp")`. With `dark=true` it adds a `<source media="(prefers-color-scheme: dark)" srcset="/img/NAME-dark.webp">` and `data-zoom-dark`.

- [ ] **Step 1: The shortcode**

```jinja
{% set meta = get_image_metadata(path="img/" ~ name ~ ".webp") %}
<figure class="shot">
  <a href="/img/{{ name }}.webp" data-zoom{% if dark %} data-zoom-dark="/img/{{ name }}-dark.webp"{% endif %}>
    <picture>
      {% if dark %}<source media="(prefers-color-scheme: dark)" srcset="/img/{{ name }}-dark.webp">{% endif %}
      <img src="/img/{{ name }}.webp" alt="{{ alt }}" width="{{ meta.width }}" height="{{ meta.height }}" loading="lazy">
    </picture>
  </a>
  {% if caption %}<figcaption>{{ caption | markdown(inline=true) | safe }}</figcaption>{% endif %}
</figure>
```

(`get_image_metadata` resolves paths relative to `static/` or `content/`; confirm with a build, adjust the prefix if needed.)

- [ ] **Step 2: The viewer dialog in `base.html`**

```html
<dialog class="viewer" id="viewer" aria-label="Screenshot">
  <button type="button" class="viewer-close" data-viewer-close aria-label="Close">✕</button>
  <button type="button" class="viewer-step prev" data-viewer-step="-1" aria-label="Previous screenshot">‹</button>
  <div class="viewer-stage"><img alt=""></div>
  <button type="button" class="viewer-step next" data-viewer-step="1" aria-label="Next screenshot">›</button>
  <p class="viewer-caption" aria-live="polite"></p>
</dialog>
```

- [ ] **Step 3: Viewer behaviour in `site.js`**

```js
  // Screenshots open large in a dialog. Leaving: the close button, Esc,
  // a click outside the image, or Back. Without this, the link opens the
  // image itself.
  const viewer = document.getElementById("viewer");
  const zoomable = [...document.querySelectorAll("a[data-zoom]")];
  if (viewer && zoomable.length) {
    const stage = viewer.querySelector(".viewer-stage");
    const image = stage.querySelector("img");
    const caption = viewer.querySelector(".viewer-caption");
    const dark = matchMedia("(prefers-color-scheme: dark)");
    let index = 0;
    let opener = null;
    const show = (i) => {
      index = (i + zoomable.length) % zoomable.length;
      const link = zoomable[index];
      const thumb = link.querySelector("img");
      image.src = (dark.matches && link.dataset.zoomDark) || link.href;
      image.alt = thumb?.alt ?? "";
      caption.textContent = link.closest("figure")?.querySelector("figcaption")?.textContent.trim() ?? "";
      viewer.classList.remove("actual");
      stage.scrollTo(0, 0);
      viewer.querySelectorAll("[data-viewer-step]").forEach((b) => (b.hidden = zoomable.length < 2));
    };
    const open = (i) => {
      opener = zoomable[i];
      show(i);
      viewer.showModal();
      history.pushState({ viewer: true }, "");
    };
    // Closing always goes through history, so Back and the other ways
    // leave the same single entry behind.
    const close = () => { if (history.state?.viewer) history.back(); else finish(); };
    const finish = () => {
      if (viewer.open) viewer.close();
      opener?.focus();
    };
    addEventListener("popstate", () => { if (viewer.open) finish(); });
    zoomable.forEach((link, i) =>
      link.addEventListener("click", (event) => {
        if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
        event.preventDefault();
        open(i);
      }),
    );
    viewer.addEventListener("cancel", (event) => { event.preventDefault(); close(); });
    viewer.querySelector("[data-viewer-close]").addEventListener("click", close);
    viewer.querySelectorAll("[data-viewer-step]").forEach((b) =>
      b.addEventListener("click", () => show(index + Number(b.dataset.viewerStep))),
    );
    viewer.addEventListener("keydown", (event) => {
      if (event.key === "ArrowRight") show(index + 1);
      if (event.key === "ArrowLeft") show(index - 1);
    });
    // Outside the image (backdrop, stage padding, caption area) closes.
    viewer.addEventListener("click", (event) => {
      if (event.target === viewer || event.target === stage) close();
    });
    // Click the image: fitted ↔ actual size, zoomed around where you clicked.
    image.addEventListener("click", (event) => {
      const fx = event.offsetX / image.clientWidth;
      const fy = event.offsetY / image.clientHeight;
      viewer.classList.toggle("actual");
      if (viewer.classList.contains("actual")) {
        stage.scrollTo(fx * image.scrollWidth - stage.clientWidth / 2, fy * image.scrollHeight - stage.clientHeight / 2);
      }
    });
    // Drag to pan when at actual size.
    let drag = null;
    stage.addEventListener("pointerdown", (event) => {
      if (!viewer.classList.contains("actual") || event.pointerType !== "mouse") return;
      drag = { x: event.clientX, y: event.clientY, left: stage.scrollLeft, top: stage.scrollTop, moved: false };
      stage.setPointerCapture(event.pointerId);
    });
    stage.addEventListener("pointermove", (event) => {
      if (!drag) return;
      const dx = event.clientX - drag.x;
      const dy = event.clientY - drag.y;
      if (Math.abs(dx) + Math.abs(dy) > 4) drag.moved = true;
      stage.scrollTo(drag.left - dx, drag.top - dy);
    });
    stage.addEventListener("pointerup", () => {
      if (drag?.moved) image.addEventListener("click", (e) => e.stopImmediatePropagation(), { capture: true, once: true });
      drag = null;
    });
    // Swipe between screenshots when fitted.
    let touch = null;
    stage.addEventListener("touchstart", (event) => {
      if (event.touches.length === 1 && !viewer.classList.contains("actual")) touch = event.touches[0].clientX;
    }, { passive: true });
    stage.addEventListener("touchend", (event) => {
      if (touch === null) return;
      const dx = event.changedTouches[0].clientX - touch;
      touch = null;
      if (Math.abs(dx) > 60) show(index + (dx < 0 ? 1 : -1));
    });
  }
```

- [ ] **Step 4: Viewer styles**

```css
/* Screenshot viewer */
a[data-zoom] { display: block; cursor: zoom-in; border-radius: 0.8rem; }
a[data-zoom]:focus-visible { outline: 3px solid var(--lamp); outline-offset: 3px; }
.viewer {
  width: 100vw; height: 100dvh; max-width: none; max-height: none; margin: 0; padding: 0;
  border: 0; background: transparent; color: #fff; overflow: hidden;
}
.viewer::backdrop { background: rgb(12 20 19 / 0.92); }
.viewer-stage {
  position: absolute; inset: 3.5rem 4rem 3.5rem; display: grid; place-items: center;
  overflow: auto; touch-action: pinch-zoom pan-x pan-y; overscroll-behavior: contain;
}
.viewer-stage img {
  max-width: 100%; max-height: 100%; width: auto; height: auto; object-fit: contain;
  border-radius: 0.6rem; box-shadow: 0 1rem 3rem rgb(0 0 0 / 0.5); cursor: zoom-in; background: #fff;
}
.viewer.actual .viewer-stage { place-items: start; }
.viewer.actual .viewer-stage img { max-width: none; max-height: none; cursor: grab; }
.viewer-close, .viewer-step {
  position: absolute; display: grid; place-items: center; width: 3rem; height: 3rem; border: 0; border-radius: 50%;
  font: inherit; font-size: 1.4rem; font-weight: 800; color: #fff; background: rgb(255 255 255 / 0.14); cursor: pointer;
}
.viewer-close:hover, .viewer-step:hover { background: rgb(255 255 255 / 0.26); }
.viewer-close:focus-visible, .viewer-step:focus-visible { outline: 3px solid var(--lamp); }
.viewer-close { top: 0.75rem; right: 0.75rem; }
.viewer-step { top: 50%; transform: translateY(-50%); font-size: 2rem; }
.viewer-step.prev { left: 0.5rem; }
.viewer-step.next { right: 0.5rem; }
.viewer-caption { position: absolute; left: 1rem; right: 1rem; bottom: 0.9rem; text-align: center; color: var(--haint-2); font-size: 0.95rem; }
@media (max-width: 40rem) {
  .viewer-stage { inset: 3.5rem 0.5rem 4rem; }
  .viewer-step { top: auto; bottom: 0.5rem; transform: none; }
}
@media (prefers-reduced-motion: no-preference) {
  .viewer[open] { animation: viewer-in 0.18s ease-out; }
  @keyframes viewer-in { from { opacity: 0; } }
}
```

- [ ] **Step 5: Check the viewer (Review Focus 2)**

Use one test page with three `shot` shortcodes (one with `dark=true`) — or the landing page once Task 11 converts the gallery. Build, serve, and in the preview browser verify each and record the result:
1. Click image → dialog opens; `history.length` grew by 1.
2. ✕ closes; focus is on the link that opened it (`document.activeElement === link`); `location.href` unchanged; `history.state` not `{viewer:true}`.
3. Open, → → ← (three images) then Back once → dialog closed, still on the same page (not the previous page).
4. Open, Esc → closed; open, click the dark area → closed; open, click the image → actual size, click again → fitted; drag pans at actual size and doesn't toggle.
5. Dark appearance: opening the `dark=true` image shows `-dark.webp`.
6. 375 px viewport: buttons at the bottom, image fits; `preview_press` Tab cycles only inside the dialog.
7. Cmd-click still opens the image in a new tab (event not prevented).
8. JavaScript disabled (serve and `curl` the HTML: `href` points at the full image) — the link opens the image.

- [ ] **Step 6: Commit**

```bash
rtk git add -A website
rtk git commit -m "docs(website): open screenshots large, zoom in, and leave with Esc, Back or a click"
```

---

### Task 5: API reference page generated from the code

**Files:**
- Modify: `src/automations/api.rs` (test module), `.dagger/main.dang` (already includes `website` from Task 1), `flake.nix`
- Create (generated): `website/content/docs/integrations/automation-api.md`

**Interfaces:**
- Consumes: `reference()` in `src/automations/api.rs:410`.
- Produces: `website/content/docs/integrations/automation-api.md` with `weight = 4` (Task 9 orders the other integration pages around it: incoming 1, outgoing 2, automations 3, API 4, AI and MCP 5).

- [ ] **Step 1: Write the failing test**

In the `tests` module of `src/automations/api.rs`:

```rust
    /// The documentation's API page is `reference()` with Zola front matter.
    fn website_page() -> String {
        let reference = reference();
        let body = reference
            .strip_prefix("# Sideporch automation API\n\n")
            .expect("the reference starts with its title");
        format!(
            "+++\n\
             title = \"Automation API\"\n\
             description = \"Every sideporch.* function and the tables handlers receive.\"\n\
             weight = 4\n\
             +++\n\n\
             <!-- Generated from src/automations/api.rs. Change the API there, then run\n     \
             SIDEPORCH_BLESS=1 cargo test website_page_is_current -->\n\n\
             {body}"
        )
    }

    #[test]
    fn website_page_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("website/content/docs/integrations/automation-api.md");
        let expected = website_page();
        if std::env::var_os("SIDEPORCH_BLESS").is_some() {
            std::fs::write(&path, &expected).expect("write the API page");
            return;
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            actual == expected,
            "{} is out of date; run SIDEPORCH_BLESS=1 cargo test website_page_is_current",
            path.display()
        );
    }
```

- [ ] **Step 2: Run it and see it fail**

Run: `rtk cargo test --lib website_page_is_current`
Expected: FAIL, "… automation-api.md is out of date".

- [ ] **Step 3: Generate the page**

Run: `SIDEPORCH_BLESS=1 rtk cargo test --lib website_page_is_current && rtk cargo test --lib website_page_is_current`
Expected: PASS both times; the file exists and starts with `+++`.

- [ ] **Step 4: Make sure it renders**

`rtk mise run site-build`; open `/docs/integrations/automation-api/`. Expected: Functions list and a heading per table; `zola check` passes (Markdown from `reference()` contains no Zola shortcode syntax like `{{`; if it does, wrap the body in `{% raw %}…{% endraw %}` in `website_page()` and rebless).

- [ ] **Step 5: Keep Nix and Dagger builds able to run the test**

`flake.nix` fileset: add `./website/content/docs/integrations/automation-api.md` to the `unions` list. Dagger already mounts `website` (Task 1 Step 7).

Run: `rtk nix flake check --print-build-logs` (Nix is installed on this machine).
Expected: the package builds and its tests, including `website_page_is_current`, pass.

- [ ] **Step 6: Commit**

```bash
rtk git add src/automations/api.rs website/content/docs/integrations/automation-api.md flake.nix
rtk git commit -m "test: keep the documentation's automation API page generated from the code"
```

---

### Tasks 6–9: Documentation content

Shared rules for every page in Tasks 6–9:

- Front matter: `title`, `description` (one sentence, used in cards and search), `weight`.
- First paragraph says what the page helps you do. Headings are tasks or things ("Change a channel's settings", "Trust levels"), not "Overview".
- Screenshots through `{{ shot(…) }}`, callouts through `{% note(…) %}`.
- Link between pages with `@/docs/…/page.md` (Zola checks these).
- **Verify before writing.** For each page, open the listed source files and confirm names, defaults, numbers and UI labels. When the README disagrees with the code, the code wins; list each such difference in the task's commit message body and in the final report.
- After each task: `rtk mise run site-check` passes (internal links as errors), then read every new page in the browser at 1440 and 375 px.

### Task 6: Get started

**Files:** Create `website/content/docs/get-started/{what-is-sideporch,try-it,install,run-on-a-server,update}.md`

- [ ] **Step 1: Verify facts**

Check and note: CLI options, env vars and defaults in `src/main.rs` (`--listen` default `127.0.0.1:8080`, `--data` default `sideporch-data`, `--public-url`, `--require-setup-link`, subcommands `setup-link`, `restore`); Docker image env (`SIDEPORCH_LISTEN=0.0.0.0:8080`, `/data`, user 10001 from `.dagger/main.dang`); `install.sh` variables (`SIDEPORCH_VERSION`, `SIDEPORCH_INSTALL_DIR`, `SIDEPORCH_DOWNLOAD_URL`) and install location; NixOS module options in `flake.nix` (`enable`, `package`, `listen`, `publicUrl`, and any others below line 80); upgrade backups in `src/db.rs` (directory `upgrade-backups/`, "newest three", file name pattern); `/healthz` exists in `src/routes.rs`.

- [ ] **Step 2: Write the pages**

1. `what-is-sideporch.md` (weight 1): README lines 24–29 (Why) and 317–323 (Good to know), plus the status note. Ends with links to Try it and Install.
2. `try-it.md` (2): Docker and Homebrew one-liners (README 31–46), what happens on first open, `localhost` vs `127.0.0.1`, inviting people. Note: "To keep it, install it on a server."
3. `install.md` (3): opens with **On a server, use the install script** — the `curl … | sh` line, what it does (detects system, verifies `SHA256SUMS`, installs to `/usr/local/bin` or `~/.local/bin`), its variables, reading the script first (`curl -fsSL …/install.sh -o install.sh; less install.sh; sh install.sh`). Then "To try it or run it in containers": Docker (with a `docker compose` example: image, port, volume, `restart: unless-stopped`), Homebrew, Nix and the NixOS module (verified options), from source. Supported platforms (README 146).
4. `run-on-a-server.md` (4): a service user and data directory; a **systemd unit** for the script install:

   ```ini
   [Unit]
   Description=Sideporch team chat
   After=network-online.target
   Wants=network-online.target

   [Service]
   User=sideporch
   Group=sideporch
   ExecStart=/usr/local/bin/sideporch --data /var/lib/sideporch --public-url https://chat.example.com
   StateDirectory=sideporch
   Restart=on-failure
   NoNewPrivileges=true
   ProtectSystem=strict
   ProtectHome=true
   ReadWritePaths=/var/lib/sideporch
   PrivateTmp=true

   [Install]
   WantedBy=multi-user.target
   ```

   with `useradd --system --home /var/lib/sideporch sideporch`, `systemctl enable --now sideporch`, `journalctl -u sideporch`. Test the unit's syntax with `systemd-analyze verify` in a Linux container if one is available; otherwise say so in the report. Then first start and the setup link (README 170–177), options table (verified), HTTPS with Caddy (README 186–194) and an nginx block that passes `/ws` upgrades (`proxy_http_version 1.1; proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection "upgrade"; proxy_set_header Host $host; proxy_set_header X-Forwarded-Proto $scheme;` — check `src/` for which forwarded headers Sideporch reads and list only those), health check `/healthz`.
5. `update.md` (5): README 196–206, per install method (script again; `brew upgrade`; `docker pull` + recreate; Nix). Link to release notes.

- [ ] **Step 3: Check and commit**

`rtk mise run site-check`; read each page at two widths.

```bash
rtk git add website/content/docs/get-started
rtk git commit -m "docs(website): get started: try, install on a server, run and update"
```

### Task 7: Using Sideporch

**Files:** Create `website/content/docs/using/{chatting,polls,search,keeping-up,phones,read-aloud-and-dictation,themes-and-profiles,shortcuts}.md`

- [ ] **Step 1: Verify facts**

Theme names and count in `src/themes.rs`; poll syntax and kinds in `src/polls.rs` and the `/poll` parsing (grep `"/poll"` in `src/`); search filters in `src/search/query.rs` (every accepted `key:`); keyboard shortcuts in `assets/app.js` (grep `key ===`, `event.key`, and the `⌘/` help dialog in `src/views.rs`) — list every shortcut the app has, not just README's five; slash commands built in (`/remind`, `/poll`, `/help`, any others: grep `"/[a-z]+"` in `src/messages.rs`, `src/later.rs`, `src/routes.rs`); speech languages/voices/sizes and idle unload time in `src/speech.rs`/`src/speech/models.rs`; iOS version requirement stays as README says unless code says otherwise; edit time limit setting names.

- [ ] **Step 2: Write the pages**

`chatting.md` (1, README 50–60, with `shot(name="channel", dark=true)` and `shot(name="deploys")`), `polls.md` (2, README 56 expanded: the three kinds, the command forms, how instant runoff reads, with a poll screenshot crop from `channel` if Task 10 adds one, else none), `search.md` (3, README 67 as a table of filters with examples, `shot(name="search")`), `keeping-up.md` (4, "Activity, saved, reminders and send later", README 62–66), `phones.md` (5, README 83–94, `shot(name="phones")`), `read-aloud-and-dictation.md` (6, README 96–103, for everyone; model download points to the community page), `themes-and-profiles.md` (7, README 105–109 + themes list, `shot(name="themes")` from Task 10), `shortcuts.md` (8, full table; `shot(name="switcher")`).

- [ ] **Step 3: Check and commit**

```bash
rtk git add website/content/docs/using
rtk git commit -m "docs(website): using Sideporch: chat, polls, search, phones and more"
```

### Task 8: Run a community

**Files:** Create `website/content/docs/community/{people-and-invites,sign-up-and-trust,moderation,sign-in-security,backups,move-from-slack,speech-models,server-settings,server-size}.md`; Modify `tools/loadtest/report.sh`; Delete `docs/capacity.md`

- [ ] **Step 1: Verify facts**

`src/community.rs`: sign-up modes, trust level requirements and defaults, every `Permission` variant with its UI label and default level, sign-up rate limit, new-member message limit, time-out behaviour. `src/security.rs`/`src/routes/security.rs`: sign-in policies and their labels, link lifetime (15 minutes?), recovery code count (10?). `src/backup.rs`: schedule options, retention, archive name pattern, what's inside. `src/import.rs`: what comes over. `src/routes/admin.rs`/`src/system.rs`: system page contents. `src/gifs.rs`, `src/previews.rs`: settings. Admin menu labels in `src/views.rs` (e.g. "Admin → Community", "Admin → Sign-in", "Admin → Backups", "Admin → Import", "Admin → Speech").

- [ ] **Step 2: Write the pages**

`people-and-invites.md` (1, README 109 + invite links, reset links, making admins, deactivation; `shot(name="people")`), `sign-up-and-trust.md` (2, README 117–123 minus moderation, with a permissions table), `moderation.md` (3, README 124, `shot(name="moderation")`), `sign-in-security.md` (4, README 128–135 + SMTP setup fields; `shot(name="sign-in")`), `backups.md` (5, README 208–220; `shot(name="backups")`), `move-from-slack.md` (6, README 222–224), `speech-models.md` (7, README 98–103 admin side), `server-settings.md` (8, "GIFs, link previews and the system page"), `server-size.md` (9, all of `docs/capacity.md`, links to `tools/loadtest/` on GitHub by absolute URL).

- [ ] **Step 3: Move the capacity document**

`git rm docs/capacity.md`. In `tools/loadtest/report.sh`, change any mention of `docs/capacity.md` to `website/content/docs/community/server-size.md`; check `tools/loadtest/README*` too (`rtk grep -rn capacity tools/`).

- [ ] **Step 4: Check and commit**

```bash
rtk git add -A website/content/docs/community docs/capacity.md tools/loadtest
rtk git commit -m "docs(website): run a community: people, trust, moderation, sign-in, backups and server size"
```

### Task 9: Integrations

**Files:** Create `website/content/docs/integrations/{incoming-webhooks,outgoing-webhooks,automations,ai-and-mcp}.md`

- [ ] **Step 1: Verify facts**

`src/webhook.rs` (accepted fields, `channel`, `username`, `icon_url`, attachments), `src/outgoing.rs` (payload fields, `token`, `response_type`), `src/automations/api.rs` (events list, limits — link to the API page rather than repeating function lists), `src/ai.rs` (providers), `src/mcp.rs` (tool names, token creation label, "start switched off").

- [ ] **Step 2: Write the pages**

`incoming-webhooks.md` (1, README 226–236, with Gatus and a Grafana contact point example and a plain `curl -X POST -H 'Content-Type: application/json' -d '{"text":"Hello"}' …` test), `outgoing-webhooks.md` (2, README 238 with the JSON payload as a code block from `src/outgoing.rs`), `automations.md` (3, README 240–291; `shot(name="automation")`; links to `@/docs/integrations/automation-api.md`), `ai-and-mcp.md` (5, README 293–303).

- [ ] **Step 3: Docs home**

Fill `content/docs/_index.md` body: one sentence, then the section cards (template does the cards), plus "New here? Start with [Try it](@/docs/get-started/try-it.md)."

- [ ] **Step 4: Check and commit**

```bash
rtk git add website/content/docs
rtk git commit -m "docs(website): integrations: webhooks, automations, AI and MCP"
```

---

### Task 10: Screenshots: one copy, retaken, and a task to do it

**Files:**
- Modify: `scripts/screenshots.mjs`, `mise.toml`, `AGENTS.md` (screenshot line)
- Create: `scripts/screenshots.sh`
- Replace: `website/static/img/*.webp`; Delete: `docs/screenshots/`

**Interfaces:**
- Produces: `website/static/img/{channel,channel-dark,deploys,automation,switcher,search,moderation,phones,themes,people,sign-in,backups}.webp`, used by Tasks 6–9 and 11 and the README.

- [ ] **Step 1: New views in `screenshots.mjs`**

Before the `console.log` at the end, add (URLs: confirm each route in `src/routes.rs`; adjust names to the real paths):

```js
{
  // Appearance: the theme picker.
  const page = await view();
  await page.goto(`${base}/settings/appearance`);
  await shot(page, "themes");
  await page.close();
}
{
  const page = await view();
  await page.goto(`${base}/people`);
  await shot(page, "people");
  await page.close();
}
{
  const page = await view({ viewport: { width: 1360, height: 1040 } });
  await page.goto(`${base}/admin/sign-in`);
  await shot(page, "sign-in");
  await page.close();
}
{
  const page = await view();
  await page.goto(`${base}/admin/backups`);
  await shot(page, "backups");
  await page.close();
}
```

Replace the header's manual steps with: "Run `mise run screenshots`; it starts a server, runs this script and writes WebP files into website/static/img/."

- [ ] **Step 2: `scripts/screenshots.sh`**

```sh
#!/bin/sh
# Retake every screenshot: build Sideporch, seed a fresh server with a demo
# team (screenshots.mjs), and write WebP files to website/static/img/.
# Needs node, cwebp and a Chromium (CHROME, or Playwright's headless shell).
set -eu
cd "$(dirname "$0")/.."
command -v cwebp >/dev/null || { echo "needs cwebp (brew install webp)" >&2; exit 1; }
cargo build --release --locked
data=$(mktemp -d)
shots=$(mktemp -d)
work=$(mktemp -d)
port=18790
target/release/sideporch --listen "127.0.0.1:$port" --data "$data" &
server=$!
trap 'kill "$server" 2>/dev/null; rm -rf "$data" "$shots" "$work"' EXIT HUP INT TERM
until curl -fsS "http://127.0.0.1:$port/healthz" >/dev/null 2>&1; do sleep 0.2; done
(cd "$work" && npm init -y >/dev/null && npm install --no-save --silent playwright-core)
NODE_PATH="$work/node_modules" node --preserve-symlinks scripts/screenshots.mjs "http://127.0.0.1:$port" "$shots"
for png in "$shots"/*.png; do
  name=$(basename "$png" .png)
  case "$name" in
    phone | phone-home) continue ;;
    phones) cwebp -quiet -q 84 -resize 1100 0 "$png" -o "website/static/img/$name.webp" ;;
    *) cwebp -quiet -q 82 -resize 1600 0 "$png" -o "website/static/img/$name.webp" ;;
  esac
done
echo "Wrote $(ls "$shots"/*.png | wc -l | tr -d ' ') screenshots to website/static/img/"
```

ES module imports ignore `NODE_PATH`; if `import "playwright-core"` fails to resolve, instead symlink: `ln -s "$work/node_modules" scripts/node_modules` inside the trap-cleaned run (add `rm -f scripts/node_modules` to the trap), and keep `scripts/node_modules` in `.gitignore`.

`mise.toml`:

```toml
[tasks.screenshots]
description = "Retake the website's and README's screenshots from a fresh demo server"
run = "sh scripts/screenshots.sh"
```

- [ ] **Step 3: Retake**

Run: `rtk mise run screenshots` (with `CHROME` set to a Playwright headless shell from `~/Library/Caches/ms-playwright/chromium_headless_shell-1243/…/chrome-headless-shell` if the default launch fails).
Expected: the script's final JSON has `"errors": []`; 12 WebP files updated.

- [ ] **Step 4: Review every screenshot**

Open each file (Read tool shows images). For each, check against the current UI: no error banners, demo content visible, no half-loaded Mermaid, nothing cut off, dark one is dark. Fix the script and rerun for any bad one.

- [ ] **Step 5: One copy**

`git rm -r docs/screenshots`; point README image paths at `website/static/img/…` (done fully in Task 12; do the path swap now so nothing breaks in between). Update `AGENTS.md` line about `docs/screenshots/` to: "`website/static/img/`: every screenshot (README and site). `mise run screenshots` retakes them from a fresh demo server; do so when the interface changes visibly."

- [ ] **Step 6: Commit**

```bash
rtk git add -A scripts mise.toml website/static/img docs/screenshots README.md AGENTS.md .gitignore
rtk git commit -m "docs: retake every screenshot from the current interface, kept once for the site and README"
```

---

### Task 11: Landing page

**Files:** Modify `website/templates/index.html`, `website/templates/base.html` (nav, meta), `website/static/style.css`, `website/static/site.js` (nothing new expected); Create `website/static/og.png`

- [ ] **Step 1: Navigation**

In `base.html`'s nav: Features (`/#features`), Docs (`/docs/`), Install (`/#install`), GitHub, search button (Task 3). At ≤40rem hide Features (as today) and show search as an icon-sized button.

- [ ] **Step 2: Hero**

Under the lede, before the Docker line: two links styled as buttons — primary "Get started" → `/docs/get-started/try-it/`, secondary "Install on a server" → `/docs/get-started/install/`. Keep the copyline; change its label to "Or try it now with Docker:".

- [ ] **Step 3: Copy that is true**

Replace the `<li><strong>Upgrades itself</strong> when you install a new version, and keeps a copy of your data first.</li>` with `<li><strong>Upgrading is replacing one file.</strong> Your data moves to the new version on its own, and a copy is kept first.</li>`. Re-read every other landing sentence against Tasks 6–9's verified facts (31 languages, 4,800/9,600/12,800, "20 themes" if mentioned, etc.) and fix differences.

- [ ] **Step 4: Gallery**

Replace each gallery `figure` with the Task 4 markup (by hand in the template, not the shortcode, which is for content). Add `switcher` ("Jump anywhere with ⌘K") and `phones` ("On phones, installed like an app, with notifications") figures. Order: channel (wide), deploys, search, switcher, moderation, automation, phones (wide, centered, max-width 40rem). Every image zoomable.

- [ ] **Step 5: Install section and links (Review Focus 1)**

Keep `id="features"` and `id="install"`. Reorder `.ways`: **Script** first with a note "recommended for servers", then Docker, Homebrew, Nix. Replace the README link paragraph with links to `/docs/get-started/run-on-a-server/`, `/docs/get-started/update/`, `/docs/community/backups/`, `/docs/community/move-from-slack/`. "How it was measured" → `/docs/community/server-size/`. Footer gains "Docs".

- [ ] **Step 6: Social preview**

Make `static/og.png` at 1200×630: render with the preview browser (`preview_resize` 1200×630 on a local HTML file showing the logo, "Sideporch", the tagline "Your own chat, on your own porch." and a crop of `channel.webp` on the dusk gradient) and save the screenshot; or with Playwright in `/tmp`. Keep it under 300 KB. In `base.html`: `og:image` = `{{ config.base_url }}/og.png`, `og:image:width`/`height`, `og:image:alt`, `twitter:card` = `summary_large_image`; `og:title`/`og:description`/`og:url` per page from the blocks.

- [ ] **Step 7: Check the landing page**

Serve the build; at 1440, 768, 375 and 320 px, light and dark:
- no sideways scroll (`scrollWidth <= innerWidth`);
- `/#install` and `/#features` scroll to their sections;
- all gallery images open in the viewer, ←/→ go through all seven;
- tab order: skip nothing, visible focus everywhere;
- hero animation still plays and respects reduced motion (`preview_evaluate` with emulated media if available, else check the CSS path);
- Lighthouse-style basics: every `img` has `alt`, `width`, `height`; one `h1`.

- [ ] **Step 8: Commit**

```bash
rtk git add -A website
rtk git commit -m "docs(website): a landing page that leads into the docs, with every screenshot zoomable"
```

---

### Task 12: README, AGENTS.md and the decision record

**Files:** Modify `README.md`, `AGENTS.md`; Create a vrdx record in `decisions/`

- [ ] **Step 1: Slim README**

Keep lines 1–29 (logo, pitch, nav links rewritten to: Docs · Try it · Install · Features, hero picture with `website/static/img/` paths, status, Why). Then:
- `## Try it` (as now, 31–46, last sentence links to `https://sideporch.app/docs/get-started/install/`).
- `## Install` — two lines: the script command "for servers", then "Docker, Homebrew, Nix and running it for real: [Install](https://sideporch.app/docs/get-started/install/) and [Run it on a server](…)".
- `## Features` — the landing page's three feature groups as short bullet lists, each bullet linking to its docs page.
- `## Documentation` — one line: everything else lives at https://sideporch.app/docs/.
- `## Develop`, `## Decisions`, `## License` unchanged, plus under Develop: "The website and docs live in `website/` (`mise run site`)."

- [ ] **Step 2: Old anchors (Review Focus 1)**

Make sure `#try-it` and `#install` still exist as headings; headings that moved (`#run-it`, `#update`, `#back-up-and-move`, `#move-from-slack`, `#automations`, `#good-to-know`, `#how-big-a-server`) are named in the Documentation section as links ("Running it, updates, backups, moving from Slack, automations …"), so a reader landing on a dead anchor sees them on screen. Check: `rtk grep -n "^## " README.md`.

- [ ] **Step 3: AGENTS.md**

Add to Layout: "`website/`: sideporch.app, built with Zola and Pagefind (`mise run site`). `website/content/docs/` is the documentation; the README only links to it. When behaviour or the interface changes, update the matching docs page in the same commit. `website/content/docs/integrations/automation-api.md` is generated: run `SIDEPORCH_BLESS=1 cargo test website_page_is_current` after changing `src/automations/api.rs`." Update the `tools/loadtest/` line to the new capacity page path.

- [ ] **Step 4: Decision record**

Run `vrdx guide` and `vrdx context "website documentation"`, then create the record (title: "Build sideporch.app with Zola and Pagefind and keep the documentation there"): Decision (Zola + Pagefind pinned by mise, Vercel builds with the locked tools, docs are the single source and the README links in, the API page is generated), Context (Niklas's request of 2026-09-29; alternatives Starlight/Astro — best out-of-the-box docs UX but a Node dependency tree; mdBook — separate generic look; hand-written HTML — unmaintainable), Consequences (templates are ours to maintain; the Vercel build downloads mise; docs describe `main`). Relate it to the static-binary and frontend records. Run `rtk vrdx validate`.

- [ ] **Step 5: Commit**

```bash
rtk git add README.md AGENTS.md decisions
rtk git commit -m "docs: move the documentation to sideporch.app and keep the README short"
```

---

### Task 13: Full check

- [ ] **Step 1: All checks**

Run: `df -h ~` first, then `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 rtk mise run check` and `rtk mise run site-check`.
Expected: both pass. External link warnings from `zola check` are read and fixed if they are ours.

- [ ] **Step 2: CI pipeline**

Run: `rtk mise run ci` if a container engine is running (`container system status` or `colima status`); otherwise report that it was not run locally.
Expected: pass, including the new `site` step.

- [ ] **Step 3: Whole-site browser pass (Review Focus 5)**

Serve `website/public`. At 1440 and 375 px, light and dark: landing page, docs home, one page from each section, the API page, 404. On each: no sideways scroll; code blocks scroll inside; tables scroll inside; search finds "restore", "passkey", "gatus", "sideporch.cron"; viewer opens and closes on a docs page.

- [ ] **Step 4: Clean up and push**

`rtk mise run clean-debug`; `rm -rf website/public`; remove any `/tmp` directories created. `rtk git status` clean. Push `main` (standing authorization) and report: commits, what Vercel needs (preset "Other", root directory `website`, no dashboard build override), README-vs-code differences found, anything not verified (Nix, Dagger, systemd-analyze).
