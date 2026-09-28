#!/bin/sh
# Regenerate assets/vendor/emoji.tsv from GitHub's gemoji (MIT).
# Columns: emoji, category, aliases (space-separated), tags, description.
set -eu
revision=${1:-master}
curl -fsSL "https://raw.githubusercontent.com/github/gemoji/$revision/db/emoji.json" |
  jq -r '.[] | [.emoji, .category, (.aliases | join(" ")), (.tags | join(" ")), .description] | @tsv' \
  > assets/vendor/emoji.tsv
wc -l assets/vendor/emoji.tsv
