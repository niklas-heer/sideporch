# sideporch.app

Sideporch's website: static HTML and CSS, with one small script (`porch.js`)
that plays the conversation in the hero once and copies the install
command. There is no build step. Vercel serves this directory (project root
directory `website`) at <https://sideporch.app>.

The page uses the app's own palette and typeface. The logo, fonts and
screenshots are copies from `assets/` and `docs/screenshots/`; copy them again
when those change (`scripts/screenshots.mjs` retakes the screenshots).

Vercel deploys when anything in this directory changed since its last
deployment (`ignoreCommand` in `vercel.json`), so pushing several commits at
once still deploys website changes that aren't in the last one.

Preview locally with any static server, for example `python3 -m http.server -d website`.
