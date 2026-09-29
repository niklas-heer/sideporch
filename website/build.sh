#!/bin/sh
# Build the site into website/public with its search index. Set SITE_URL
# to serve the build from somewhere other than https://sideporch.app.
set -eu
cd "$(dirname "$0")"
if [ -n "${SITE_URL:-}" ]; then
  zola build --base-url "$SITE_URL"
else
  zola build
fi
pagefind --site public
