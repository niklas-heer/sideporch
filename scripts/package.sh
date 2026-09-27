#!/bin/sh
# Archive an already built executable for a release. No upload or Git changes.
# Usage: package.sh VERSION TARGET [BINARY] [DESTINATION]
set -eu

version=${1:?usage: package.sh VERSION TARGET [BINARY] [DESTINATION]}
target=${2:?usage: package.sh VERSION TARGET [BINARY] [DESTINATION]}
binary=${3:-target/$target/release/sideporch}
destination=${4:-dist}

if ! printf '%s\n' "$version" | LC_ALL=C grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "release version must be MAJOR.MINOR.PATCH" >&2
  exit 1
fi
case "$target" in
  x86_64-unknown-linux-musl|aarch64-unknown-linux-musl|x86_64-apple-darwin|aarch64-apple-darwin) ;;
  *) echo "unsupported release target: $target" >&2; exit 1 ;;
esac
if [ ! -x "$binary" ]; then
  echo "missing executable: $binary" >&2
  exit 1
fi
# Linux archives must hold a static executable that needs no system libc.
case "$target" in
  *-linux-musl)
    if command -v file >/dev/null 2>&1 && ! file "$binary" | grep -q 'statically linked'; then
      echo "$binary is not statically linked" >&2
      exit 1
    fi
    ;;
esac

mkdir -p "$destination"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM
cp "$binary" "$stage/sideporch"
cp LICENSE README.md "$stage/"
mkdir -p "$stage/licenses"
cp assets/fonts/OFL.txt "$stage/licenses/atkinson-hyperlegible-next-OFL.txt"
archive="sideporch-$version-$target.tar.gz"
tar -czf "$destination/$archive" -C "$stage" sideporch LICENSE README.md licenses
(
  cd "$destination"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$archive" > "$archive.sha256"
  else
    shasum -a 256 "$archive" > "$archive.sha256"
  fi
)
printf '%s\n' "$destination/$archive"
