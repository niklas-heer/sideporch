#!/bin/sh
# Signs a release's SHA256SUMS with the release key, so Sideporch can check
# an update came from a release before installing it (src/updates/).
#
#   SIDEPORCH_RELEASE_SIGNING_KEY=<minisign secret key> release-sign.sh VERSION DIRECTORY
#
# The trusted comment names the version; Sideporch refuses a signature made
# for another one. The signature is checked against website/static/sideporch.pub,
# the key built into Sideporch, before the release is published.
set -eu
version=${1:?usage: release-sign.sh VERSION DIRECTORY}
assets=${2:?usage: release-sign.sh VERSION DIRECTORY}
if [ -z "${SIDEPORCH_RELEASE_SIGNING_KEY:-}" ]; then
  echo "release-sign: SIDEPORCH_RELEASE_SIGNING_KEY is not set" >&2
  exit 1
fi
key=$(mktemp)
trap 'rm -f "$key"' EXIT HUP INT TERM
printf '%s\n' "$SIDEPORCH_RELEASE_SIGNING_KEY" > "$key"
minisign -S -s "$key" -m "$assets/SHA256SUMS" -t "sideporch $version" -c "Sideporch $version release checksums"
minisign -V -p "$(dirname "$0")/../website/static/sideporch.pub" -m "$assets/SHA256SUMS"
