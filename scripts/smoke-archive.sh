#!/bin/sh
# Verify a release archive and, when this machine can run it, start the
# server from it and check that it answers.
# Usage: smoke-archive.sh VERSION TARGET ASSET_DIRECTORY
set -eu
version=${1:?usage: smoke-archive.sh VERSION TARGET ASSET_DIRECTORY}
target=${2:?usage: smoke-archive.sh VERSION TARGET ASSET_DIRECTORY}
assets=${3:?usage: smoke-archive.sh VERSION TARGET ASSET_DIRECTORY}
archive="sideporch-$version-$target.tar.gz"
(
  cd "$assets"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "$archive.sha256"
  else
    shasum -a 256 -c "$archive.sha256"
  fi
)
temporary=$(mktemp -d)
cleanup() {
  [ -n "${server:-}" ] && kill "$server" 2>/dev/null || true
  rm -rf "$temporary"
}
trap cleanup EXIT HUP INT TERM
tar -xzf "$assets/$archive" -C "$temporary"
if ! "$temporary/sideporch" --version >/dev/null 2>&1; then
  echo "Checked $archive; this machine cannot run $target binaries"
  exit 0
fi
test "$("$temporary/sideporch" --version)" = "sideporch $version"
port=$((20000 + $$ % 20000))
"$temporary/sideporch" --data "$temporary/data" --listen "127.0.0.1:$port" > "$temporary/log" 2>&1 &
server=$!
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if curl -fsS "http://127.0.0.1:$port/healthz" >/dev/null 2>&1; then
    # A fresh server sends the first visitor to the setup page.
    curl -fsS -o /dev/null -w '%{redirect_url}' "http://127.0.0.1:$port/" | grep -q '/setup$'
    "$temporary/sideporch" setup-link --data "$temporary/data" | grep -q "/setup"
    printf 'Verified sideporch %s for %s: it starts, answers, and offers the first-account setup\n' "$version" "$target"
    exit 0
  fi
  sleep 0.5
done
cat "$temporary/log" >&2
echo "the server did not answer" >&2
exit 1
