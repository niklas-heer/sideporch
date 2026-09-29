#!/bin/sh
# Retakes every screenshot: builds Sideporch, seeds a fresh server with a
# demo team (screenshots.mjs), and writes WebP files to website/static/img/,
# which the website, the docs and the README share.
#
# Needs node, cwebp (brew install webp) and a Chromium: CHROME, or the
# headless shell Playwright installs (npx playwright install chromium-headless-shell).
set -eu
cd "$(dirname "$0")/.."
command -v node >/dev/null || { echo "screenshots: needs node" >&2; exit 1; }
command -v cwebp >/dev/null || { echo "screenshots: needs cwebp (brew install webp)" >&2; exit 1; }

cargo build --release --locked
# A short, fixed data directory: the backups page shows its path.
data=/tmp/sideporch-demo
rm -rf "$data"
mkdir -p "$data"
shots=$(mktemp -d)
modules=$(mktemp -d)
port=18790
target/release/sideporch --listen "127.0.0.1:$port" --data "$data" >"$data.log" 2>&1 &
server=$!
cleanup() {
  kill "$server" 2>/dev/null || true
  rm -rf "$data" "$data.log" "$shots" "$modules" scripts/node_modules
}
trap cleanup EXIT HUP INT TERM
until curl -fsS "http://127.0.0.1:$port/healthz" >/dev/null 2>&1; do sleep 0.2; done

# playwright-core, for this run only. Node resolves the import from the
# script's directory, so link it there.
(cd "$modules" && npm init -y >/dev/null && npm install --silent --no-audit --no-fund playwright-core)
ln -s "$modules/node_modules" scripts/node_modules
node scripts/screenshots.mjs "http://127.0.0.1:$port" "$shots"

for png in "$shots"/*.png; do
  name=$(basename "$png" .png)
  case "$name" in
    phone | phone-home) ;; # parts of phones.png
    phones) cwebp -quiet -q 84 -resize 1100 0 "$png" -o "website/static/img/$name.webp" ;;
    *) cwebp -quiet -q 82 -resize 1600 0 "$png" -o "website/static/img/$name.webp" ;;
  esac
done
echo "Wrote the screenshots to website/static/img/"
