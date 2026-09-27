#!/bin/sh
# Write SHA256SUMS from a complete set of release archives. The Homebrew
# formula in niklas-heer/homebrew-tap and install.sh both verify against it.
set -eu
version=${1:?usage: release-checksums.sh VERSION ASSET_DIRECTORY}
assets=${2:?usage: release-checksums.sh VERSION ASSET_DIRECTORY}
if ! printf '%s\n' "$version" | LC_ALL=C grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "release version must be MAJOR.MINOR.PATCH" >&2
  exit 1
fi
for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl; do
  if [ ! -f "$assets/sideporch-$version-$target.tar.gz" ]; then
    echo "missing release asset for $target" >&2
    exit 1
  fi
done
(
  cd "$assets"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum sideporch-"$version"-*.tar.gz > SHA256SUMS
  else
    shasum -a 256 sideporch-"$version"-*.tar.gz > SHA256SUMS
  fi
)
