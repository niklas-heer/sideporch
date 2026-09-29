# sideporch.app

Sideporch's website and documentation, built with [Zola](https://www.getzola.org)
and searched with [Pagefind](https://pagefind.app). Vercel serves it at
<https://sideporch.app> (project root directory `website`).

```sh
mise run site         # preview with live reload at http://127.0.0.1:1111
mise run site-build   # build into website/public, with the search index
mise run site-check   # check every link, internal and external, then build
```

To look at a build with a plain static server, give it the address:
`SITE_URL=http://127.0.0.1:8765 mise run site-build`, then
`python3 -m http.server -d website/public 8765`.

## Layout

- `content/_index.md`: the landing page; its layout is `templates/index.html`.
- `content/docs/`: the documentation, one directory per section, and one
  per group of pages inside a section (like `integrations/automations/`);
  the sidebar, breadcrumbs and previous/next links follow that nesting. Order
  pages and groups with `weight` in their front matter, and give a moved page
  `aliases` with its old URL. `content/docs/integrations/automations/api.md`
  is generated from `src/automations/api.rs`; see its first lines.
- `templates/`: `base.html` (head, navigation, footer), the landing page, the
  docs page and section layouts, and Tera components in `components/`. Pages
  use them for screenshots and callouts:
  `{{<shot name="search" alt="…" caption="…" />}}` and
  `{% <note kind="tip"> %}…{% </note> %}` (`kind` is `info`, `tip` or `warning`),
  and diagrams in Mermaid's language: `{% <diagram caption="…"> %}…{% </diagram> %}`
  (no blank lines inside). `site.js` draws them with the Mermaid build
  Sideporch ships; `vendor.sh` unpacks it into `static/vendor/` before each
  build and preview.
- `static/`: the stylesheet, `site.js` (the hero conversation, copy buttons,
  search, the docs menu and the screenshot viewer), fonts, the logo and
  `img/<version>/`, where every screenshot lives, named after the version it
  shows (`data/screenshots.toml`; `mise run screenshots` retakes them and
  updates it). The README uses them too.
  `og.png` is the 1200×630 picture link previews show; remake it when the
  look or the channel screenshot changes a lot.

## Deploying

Zola and Pagefind are pinned in the repository's `mise.toml`, with their
download checksums in `mise.lock`. Vercel (and the Dagger `site` check) run
`ci-build.sh`, which downloads the same files from the URLs in `mise.lock`,
checks their checksums and builds. It does not use mise, because mise would
check the downloads' GitHub attestations through the GitHub API, whose
unauthenticated limit Vercel's builders share. Preview deployments link to
their own address.

Vercel deploys when this directory or the tool pins changed since its last
deployment (`ignoreCommand` in `vercel.json`), so pushing several commits at
once still deploys website changes that aren't in the last one. When the
last deployment's commit isn't in Vercel's shallow clone, it builds.

To upgrade Zola or Pagefind, change `mise.toml`, then run
`GITHUB_TOKEN=$(gh auth token) mise lock --platform linux-x64,linux-arm64,macos-arm64,macos-x64`.
