# sideporch.app with documentation: design

Date: 2026-09-29. First of three projects agreed on this day, in order: the
website with documentation (this one), update checks and self-update, then
federation between instances. Each gets its own design.

## Goal

sideporch.app should be the place where people learn about Sideporch, decide
to use it, and find how to install, run, use and administer it. Today the site
is one static landing page and the documentation is a 350-line README plus
`docs/capacity.md`. Success means:

- a landing page that works as advertising: clear, fast, accessible, correct,
  in light and dark, on phones and desktops;
- documentation on the site, organised by task, searchable, with the README
  reduced to a pitch that links there;
- every screenshot can be enlarged, zoomed, and left again in obvious ways;
- every statement and screenshot matches the current code.

What Niklas asked for, 2026-09-29: recommend the `curl` install for servers
(as opposed to trying Sideporch out), move documentation to the website with
"a nice system", make images clickable and zoomable "and that you can leave",
make the site "top-notch" for advertising and documentation, and bring all
information and screenshots up to date.

Choices he made in the design conversation: Zola with Pagefind as the docs
system; the site is the single source of the documentation and the README
becomes a slim pitch.

## 1. Structure and build

`website/` becomes a [Zola](https://www.getzola.org) site:

```
website/
  config.toml            base_url https://sideporch.app, highlighting, no taxonomies
  content/
    _index.md            landing page (its layout is templates/index.html)
    docs/_index.md       docs home
    docs/<section>/_index.md and *.md   guides, ordered by weight
  templates/             base, index (landing), docs page, docs section, 404, shortcodes
  static/                fonts/, img/, logo.svg, site.js, og.png
  static/style.css       plain CSS (no Sass), organised base / landing / docs
```

- **Tool versions**: `mise.toml` pins `zola` (registry, aqua backend) and
  `github:CloudCannon/pagefind`; `mise.lock` records their checksums.
- **Tasks**: `mise run site` (`zola serve`), `mise run site-build`
  (`zola build` then `pagefind --site public`), `mise run site-check`
  (`zola check` and the build).
- **Vercel** keeps the Git integration and the existing `ignoreCommand`.
  `vercel.json` gets a `buildCommand` that installs a pinned mise, runs
  `mise install` for the two tools (verified by `mise.lock`), then
  `mise run site-build`, and `outputDirectory: "public"`. Long cache headers
  cover `/fonts`, `/img` and `/pagefind`.
- **CI**: the Dagger module gains a `site` function that builds the site and
  checks internal links; `ci` calls it. External link checking stays a local
  task (`site-check`), since remote sites flake.
- **Search**: Pagefind indexes docs pages only (`data-pagefind-body` on the
  article). Its script loads when search opens, from the search button, `/`
  or ⌘K/Ctrl+K. Without JavaScript the site works fully, minus search.

## 2. Documentation

### Sections

All under `/docs/`. Content comes from the README and `docs/capacity.md`,
rewritten as separate pages and checked against the code.

- **Get started**: what Sideporch is and isn't (including today's "Good to
  know"); try it; install (the script recommended for servers, Docker and
  Homebrew for trying it, Nix/NixOS, from source); run it on a server
  (options and environment variables, the setup link, HTTPS with Caddy and
  nginx, a systemd unit for the script install); update.
- **Using Sideporch**: chatting (channels, private channels, threads, DMs,
  Markdown and diagrams, reactions, custom emoji, GIFs, files, link previews,
  edit, delete, pin, announcement channels); polls; search; Activity, saved
  messages, reminders and send later; phones and notifications; read aloud
  and dictation; themes and profiles; keyboard shortcuts.
- **Run a community**: people and invites; sign-up, trust levels, roles,
  permissions and moderation; sign-in security (passkeys, authenticator apps,
  email over SMTP, policy); backups and restore; moving from Slack; speech
  models; GIFs, link previews and the system page; how big a server.
- **Integrations**: incoming webhooks (Gatus, Grafana, anything Slack-style);
  outgoing webhooks; automations guide; automation API reference; AI and MCP.

### Generated API reference

`src/automations/api.rs::reference()` already renders the Lua API as
Markdown. A Rust test renders it, with Zola front matter and without the H1,
and compares it to
`website/content/docs/integrations/automation-api.md`. With
`SIDEPORCH_BLESS=1` the test rewrites the file instead. A change to the API
therefore fails `mise run check` until the docs page is regenerated.

### Accuracy

Before a page is written, its claims are checked against the code: command
line options and defaults (`src/main.rs`), themes (`src/themes.rs`), speech
languages and sizes (`src/speech/`), shortcuts (`assets/app.js`), limits,
permissions (`src/community.rs`), search filters (`src/search/query.rs`),
events (`api.rs`). Differences are fixed in the docs, and noted in the final
report if they look like bugs in the code.

### README after the move

Keeps: logo and pitch, hero screenshot, status note, "Try it", a short
feature list whose items link into the docs, Develop, Decisions, License.
Everything else lives on the site. `docs/capacity.md` moves to the site;
`tools/loadtest/report.sh` and `AGENTS.md` point at the new location.
`AGENTS.md` also learns where the docs live, that the site is their source,
and that interface or behaviour changes update the matching page.

### Doc page layout

- Left sidebar with sections and pages, current page marked; a drawer on
  narrow screens.
- "On this page" contents on wide screens; every heading has an anchor.
- Previous/next links and "Edit this page on GitHub".
- Code blocks highlighted by Zola, with a copy button.
- A `note` shortcode for callouts (tip, warning).
- The site's palette and Atkinson Hyperlegible Next; light and dark follow
  the system.

## 3. Landing page, images, screenshots

### Zoomable images

Every screenshot on the landing page and in the docs is wrapped in a link to
the full image, so it opens without JavaScript. With JavaScript, `site.js`
shows it in a native `<dialog>`:

- fitted to the viewport with its caption; click or tap toggles actual size,
  then drag or scroll pans; pinch zoom works on touch screens;
- ← → and swiping move between the page's images;
- a dark screenshot is shown when the page shows the dark one;
- leaving: a large close button, Esc, a click on the backdrop, and the
  browser's Back button (opening pushes a history entry); focus returns to
  the image that opened it;
- no animation with reduced motion.

### Landing page

- Navigation: Features, Docs, Install, GitHub, and search.
- Hero: a "Get started" link to the docs next to the Docker one-liner, and
  "Install on a server".
- Copy that is true today: "Upgrades itself" becomes that upgrading means
  replacing one file and the data migrates safely, keeping a copy.
- Gallery adds the quick switcher and phones; all images zoomable.
- Install section recommends the script for servers and links to the docs.
- "How it was measured" and other GitHub links to documentation point to
  the docs.
- A 1200×630 PNG as the social preview image (`og:image`), with
  `twitter:card`.
- A 404 page in the site's style.
- Checked at 375, 768 and 1440 pixels wide, light and dark, keyboard only.

### Screenshots

- `scripts/screenshots.mjs` writes into `website/static/img/`, the one copy.
  The README uses those paths; `docs/screenshots/` is removed.
- `mise run screenshots` starts a release build on a temporary data
  directory, runs the script, converts to WebP and stops the server. The
  script's header points to the task.
- All screenshots are retaken from the current code and reviewed. New ones
  where the docs need them: sign-in settings, backups, themes, people and
  invites.

## Verification

- `mise run site-check`: builds, indexes, no broken links.
- The API reference test and the rest of `mise run check` pass.
- The built site is driven in a browser: landing page at three widths, light
  and dark; search; the image viewer with mouse, keyboard and a touch-sized
  viewport, including leaving it each way; the docs drawer on a phone width.
- Build output created during the work is cleaned up afterwards.

## Decision record

Record with vrdx: sideporch.app is built with Zola and Pagefind, versions
pinned through mise, and is the single source of the documentation.

## Out of scope

Update checks and self-update, and federation, which follow as their own
projects. Translations of the site. Versioned documentation: the site deploys from
`main`, so pages describe `main`, and anything newer than the latest release
says which version it arrives in.
