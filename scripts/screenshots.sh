#!/bin/sh
# Retakes every screenshot: builds Sideporch, seeds a fresh server with a
# demo team (screenshots.mjs), and writes WebP files to
# website/static/img/<version>/, which the website, the docs and the README
# share. The version is Cargo.toml's, or SIDEPORCH_SCREENSHOTS_VERSION when
# the screenshots show the next release. Older versions' directories are
# removed (git keeps them), and the site and README point at the new one.
#
# Needs node, cwebp (brew install webp) and a Chromium: CHROME, or the
# headless shell Playwright installs (npx playwright install chromium-headless-shell).
set -eu
cd "$(dirname "$0")/.."
command -v node >/dev/null || { echo "screenshots: needs node" >&2; exit 1; }
command -v cwebp >/dev/null || { echo "screenshots: needs cwebp (brew install webp)" >&2; exit 1; }

version=${SIDEPORCH_SCREENSHOTS_VERSION:-$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)}
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "screenshots: \"$version\" isn't a version like 1.2.3" >&2; exit 1 ;;
esac
img=website/static/img

cargo build --release --locked
# A short, fixed data directory: the backups page shows its path.
data=/tmp/sideporch-demo
rm -rf "$data"
mkdir -p "$data"
shots=$(mktemp -d)
modules=$(mktemp -d)
port=18790
# Something else on the port would end up in the screenshots.
if curl -s -o /dev/null "http://127.0.0.1:$port/"; then
  echo "screenshots: something already answers on port $port; stop it first" >&2
  exit 1
fi
# The public URL shows in invite links and on Connections. It's http, so
# cookies work without TLS here; screenshots.mjs shows it as https, as on a
# real server.
target/release/sideporch --listen "127.0.0.1:$port" --data "$data" --update-check false --public-url http://chat.porch.example >"$data.log" 2>&1 &
server=$!
cleanup() {
  kill "$server" 2>/dev/null || true
  rm -rf "$data" "$data.log" "$shots" "$modules" scripts/node_modules
}
trap cleanup EXIT HUP INT TERM
waited=0
until curl -fsS "http://127.0.0.1:$port/healthz" >/dev/null 2>&1; do
  if ! kill -0 "$server" 2>/dev/null || [ "$waited" -ge 150 ]; then
    echo "screenshots: Sideporch didn't start:" >&2
    cat "$data.log" >&2
    exit 1
  fi
  waited=$((waited + 1))
  sleep 0.2
done

# playwright-core, for this run only. Node resolves the import from the
# script's directory, so link it there.
(cd "$modules" && npm init -y >/dev/null && npm install --silent --no-audit --no-fund playwright-core)
ln -s "$modules/node_modules" scripts/node_modules
SIDEPORCH_DATA="$data" node scripts/screenshots.mjs "http://127.0.0.1:$port" "$shots"

webp=$(mktemp -d)
for png in "$shots"/*.png; do
  name=$(basename "$png" .png)
  case "$name" in
    phone | phone-home) ;; # parts of phones.png
    phones) cwebp -quiet -q 84 -resize 1100 0 "$png" -o "$webp/$name.webp" ;;
    *) cwebp -quiet -q 82 -resize 1600 0 "$png" -o "$webp/$name.webp" ;;
  esac
done
# Only the newest set stays; git history keeps the others.
find "$img" -mindepth 1 -maxdepth 1 \( -type d -o -name '*.webp' \) -exec rm -rf {} +
mv "$webp" "$img/$version"
chmod 755 "$img/$version"
sed -i.bak "s/^version = \".*\"/version = \"$version\"/" website/data/screenshots.toml
sed -i.bak -E "s#website/static/img/([0-9]+\.[0-9]+\.[0-9]+/)?#website/static/img/$version/#g" README.md
rm website/data/screenshots.toml.bak README.md.bak
echo "Wrote the screenshots of Sideporch $version to $img/$version/"
