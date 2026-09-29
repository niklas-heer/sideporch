#!/bin/sh
# Unpacks the scripts the site shares with Sideporch into static/vendor/:
# Mermaid draws the diagrams in the docs. The copy lives once, compressed,
# in assets/vendor/ (see its README); static/vendor/ is not committed.
set -eu
cd "$(dirname "$0")"
mkdir -p static/vendor
gzip -dc ../assets/vendor/mermaid-12.0.0.min.js.gz > static/vendor/mermaid.min.js
