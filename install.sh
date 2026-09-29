#!/bin/sh
# Install Sideporch from its GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/niklas-heer/sideporch/main/install.sh | sh
#
# Environment:
#   SIDEPORCH_VERSION      version to install, such as 0.1.0 (default: latest)
#   SIDEPORCH_INSTALL_DIR  where to put the binary (default: /usr/local/bin
#                          when writable, otherwise ~/.local/bin)
#   SIDEPORCH_DOWNLOAD_URL base URL of the release files (default: GitHub)
#
# Linux binaries are static: they need no libc or other system libraries.
set -eu

repo="niklas-heer/sideporch"
# Signs every release's SHA256SUMS (from 0.5.0 on); also at https://sideporch.app/sideporch.pub.
public_key="RWRtn2cj2SpGnZDtKTNsgnc8mv68NfwlwFgA+hcmOD+cWH8dSQxGkuoo"

fail() {
  echo "sideporch install: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "this script needs $1"
}

need curl
need tar
need uname

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) fail "no prebuilt binary for $(uname -s); build from source with cargo" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) fail "no prebuilt binary for $(uname -m); build from source with cargo" ;;
esac
target="$arch-$os"

version="${SIDEPORCH_VERSION:-latest}"
version="${version#v}"
if [ "$version" = "latest" ]; then
  # GitHub redirects /releases/latest to /releases/tag/vX.Y.Z.
  latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest") ||
    fail "could not find the latest release"
  version="${latest##*/v}"
fi
case "$version" in
  *[!0-9.]* | "") fail "unexpected version: $version" ;;
esac

base="${SIDEPORCH_DOWNLOAD_URL:-https://github.com/$repo/releases/download/v$version}"
archive="sideporch-$version-$target.tar.gz"

temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM

echo "Downloading Sideporch $version for $target"
curl -fsSL "$base/$archive" -o "$temporary/$archive" || fail "download failed: $base/$archive"
curl -fsSL "$base/SHA256SUMS" -o "$temporary/SHA256SUMS" || fail "download failed: $base/SHA256SUMS"
# With minisign installed, also check the checksums come from a release.
if command -v minisign >/dev/null 2>&1; then
  if curl -fsSL "$base/SHA256SUMS.minisig" -o "$temporary/SHA256SUMS.minisig" 2>/dev/null; then
    minisign -Vq -P "$public_key" -m "$temporary/SHA256SUMS" || fail "SHA256SUMS isn't signed with Sideporch's release key"
    echo "Verified the release signature"
  else
    echo "This release has no signature; releases before 0.5.0 weren't signed"
  fi
fi

expected=$(awk -v name="$archive" '$2 == name || $2 == "./" name { print $1 }' "$temporary/SHA256SUMS")
[ -n "$expected" ] || fail "SHA256SUMS has no entry for $archive"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$temporary/$archive" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "$temporary/$archive" | awk '{ print $1 }')
else
  fail "this script needs sha256sum or shasum to verify the download"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $archive"

tar -xzf "$temporary/$archive" -C "$temporary" sideporch

directory="${SIDEPORCH_INSTALL_DIR:-}"
if [ -z "$directory" ]; then
  if [ -w /usr/local/bin ]; then
    directory=/usr/local/bin
  else
    directory="$HOME/.local/bin"
  fi
fi
mkdir -p "$directory"
install -m 755 "$temporary/sideporch" "$directory/sideporch" 2>/dev/null ||
  { cp "$temporary/sideporch" "$directory/sideporch" && chmod 755 "$directory/sideporch"; }

echo "Installed $("$directory/sideporch" --version) to $directory/sideporch"
case ":$PATH:" in
  *":$directory:"*) ;;
  *) echo "Add $directory to your PATH to run it as 'sideporch'." ;;
esac
echo "Start it with: sideporch --data ./sideporch-data"
