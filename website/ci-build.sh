#!/bin/sh
# Build the site on a clean Linux machine (Vercel, Dagger) without mise:
# download Zola and Pagefind from the URLs pinned in mise.lock, check them
# against its checksums, then run build.sh. With CHECK=1, also check the
# site's internal links first.
set -eu
cd "$(dirname "$0")"
lock=../mise.lock
case "$(uname -m)" in
  x86_64) platform=linux-x64 ;;
  aarch64 | arm64) platform=linux-arm64 ;;
  *) echo "ci-build: no pinned tools for $(uname -m)" >&2; exit 1 ;;
esac

# locked TOOL FIELD: a field of TOOL's entry for this platform in mise.lock.
locked() {
  awk -v section="[tools.\"$1\".\"platforms.$platform\"]" -v field="$2" '
    $0 == section { inside = 1; next }
    /^\[/ { inside = 0 }
    inside && $1 == field { gsub(/"/, "", $3); print $3 }
  ' "$lock"
}

tools=$(mktemp -d)
trap 'rm -rf "$tools"' EXIT
for tool in github:getzola/zola github:CloudCannon/pagefind; do
  url=$(locked "$tool" url)
  checksum=$(locked "$tool" checksum)
  [ -n "$url" ] && [ -n "$checksum" ] || { echo "ci-build: mise.lock has no $platform entry for $tool" >&2; exit 1; }
  curl -fsSL "$url" -o "$tools/archive.tar.gz"
  echo "${checksum#sha256:}  $tools/archive.tar.gz" | sha256sum -c - >/dev/null ||
    { echo "ci-build: checksum mismatch for $url" >&2; exit 1; }
  tar -xzf "$tools/archive.tar.gz" -C "$tools"
  rm "$tools/archive.tar.gz"
done
export PATH="$tools:$PATH"
zola --version
pagefind --version

# Preview deployments on Vercel link to themselves, not to sideporch.app.
if [ -n "${VERCEL_ENV:-}" ] && [ "$VERCEL_ENV" != production ] && [ -n "${VERCEL_URL:-}" ]; then
  export SITE_URL="https://$VERCEL_URL"
fi
if [ "${CHECK:-}" = 1 ]; then
  zola check --skip-external-links
fi
sh build.sh
