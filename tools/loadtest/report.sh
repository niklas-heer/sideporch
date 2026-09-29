#!/bin/sh
# Turns the JSON lines from run.sh into Markdown tables, one per result file.
#
#   tools/loadtest/report.sh [tools/loadtest/results/*.jsonl]
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
[ "$#" -gt 0 ] || set -- "$HERE"/results/*.jsonl

for file in "$@"; do
  name=$(basename "$file" .jsonl)
  echo "### $name"
  echo
  echo "| People online | Messages/s | Post p95 | Seen p95 | Seen p99 | Page p95 | Server CPU (avg) | Memory (max) | Result |"
  echo "| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |"
  jq -r --argjson limit "${LIMIT_MS:-1000}" '
    def ms: if . >= 1000 then "\(. / 1000 * 10 | round / 10) s" else "\(. | round) ms" end;
    [
      .users,
      (.messages_per_s * 10 | round / 10),
      (.post_ms.p95 | ms),
      (.delivery_ms.p95 | ms),
      (.delivery_ms.p99 | ms),
      (.page_ms.p95 | ms),
      "\(.server.cpu_avg_pct | round) %",
      "\(.server.memory_max_mib | round) MiB",
      (if .post_ms.p95 <= $limit and .delivery_ms.p95 <= $limit and .delivered_ratio >= 0.99
          and .post_errors == 0 and .page_errors == 0 and .socket_errors == 0 and .resyncs == 0
       then "ok" else "too slow" end)
    ] | "| " + (map(tostring) | join(" | ")) + " |"' "$file"
  echo
done
