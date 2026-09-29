#!/bin/sh
# Measures how many people a Sideporch server of a given size can serve.
#
#   tools/loadtest/run.sh "0.5:512m 1:1g 2:2g" "25 50 100 200 400 800 1600"
#
# For each size (CPUs:memory), it starts the Sideporch image in a container
# limited to that size, creates accounts, and steps through the numbers of
# people online at once. Each step keeps everyone connected for DURATION
# seconds while each person posts a message every MESSAGE_EVERY seconds on
# average and loads a channel page every PAGE_EVERY seconds. A step passes
# when 95% of messages post and reach everyone else within LIMIT_MS, nothing
# fails, and no connection falls behind; the steps stop at the first
# failure. Results go to tools/loadtest/results/ as JSON lines, one file
# per image and size.
#
# The accounts are created once, with SEED_IMAGE, and every run starts from
# a copy of that data; SEED_IMAGE should be the oldest image under test, so
# newer ones can migrate its database.
#
# Needs Docker (such as Colima) and the load generator built for Linux:
#   docker run --rm -v "$PWD/tools/loadtest":/src -w /src rust:1-slim-bookworm \
#     cargo build --release --target-dir /src/target-linux
set -eu

SIZES=${1:-"1:1g 2:2g"}
LEVELS=${2:-"25 50 100 200 400 800 1600"}
IMAGE=${IMAGE:-ghcr.io/niklas-heer/sideporch:latest}
SEED_IMAGE=${SEED_IMAGE:-$IMAGE}
LABEL=$(printf '%s' "$IMAGE" | sed 's|.*/||; s|:|-|g')
DURATION=${DURATION:-60}
MESSAGE_EVERY=${MESSAGE_EVERY:-120}
PAGE_EVERY=${PAGE_EVERY:-300}
LIMIT_MS=${LIMIT_MS:-1000}
HERE=$(cd "$(dirname "$0")" && pwd)
RESULTS="$HERE/results"
NETWORK=sideporch-load
SERVER=sideporch-load-server
SEED=sideporch-load-seed
DATA=sideporch-load-data

mkdir -p "$RESULTS"
docker network create "$NETWORK" >/dev/null 2>&1 || true

load() {
  docker run --rm --network "$NETWORK" -v "$HERE":/work -w /work debian:bookworm-slim \
    ./target-linux/release/sideporch-load "$@"
}

# CPU (% of one core) and memory (MiB) of the server, every two seconds.
sample() {
  while :; do
    docker stats --no-stream --format '{{.CPUPerc}} {{.MemUsage}}' "$SERVER" 2>/dev/null |
      awk '{ cpu = $1; sub("%", "", cpu); mem = $2; unit = mem; gsub("[0-9.]", "", unit); sub("[A-Za-z]+", "", mem);
             if (unit == "GiB") mem *= 1024; if (unit == "KiB") mem /= 1024; print cpu, mem }'
    sleep 2
  done
}

last_level() {
  for level in $LEVELS; do :; done
  echo "$level"
}

wait_for_server() {
  for _ in $(seq 1 30); do
    if docker run --rm --network "$NETWORK" debian:bookworm-slim bash -c "exec 3<>/dev/tcp/$SERVER/8080" 2>/dev/null; then
      return 0
    fi
    sleep 1
  done
  echo "the server did not start" >&2
  exit 1
}

# Accounts and channels, created once; hashing thousands of passwords is slow.
if ! docker volume inspect "$SEED" >/dev/null 2>&1 || [ ! -f "$HERE/accounts.json" ]; then
  echo "== creating $(last_level) accounts with $SEED_IMAGE"
  docker volume rm -f "$SEED" >/dev/null 2>&1 || true
  docker volume create "$SEED" >/dev/null
  docker run --rm -v "$SEED":/data alpine chown 10001:10001 /data
  docker rm -f "$SERVER" >/dev/null 2>&1 || true
  docker run -d --name "$SERVER" --network "$NETWORK" -v "$SEED":/data "$SEED_IMAGE" >/dev/null
  wait_for_server
  load setup --url "http://$SERVER:8080" --users "$(last_level)" --channels 10 --concurrency 8 --out accounts.json
  docker stop "$SERVER" >/dev/null
  docker rm "$SERVER" >/dev/null
fi

for size in $SIZES; do
  cpus=${size%%:*}
  memory=${size#*:}
  out="$RESULTS/$LABEL-$cpus-cpu-$memory.jsonl"
  : >"$out"
  echo "== $IMAGE: $cpus CPU, $memory memory"
  docker rm -f "$SERVER" >/dev/null 2>&1 || true
  docker volume rm -f "$DATA" >/dev/null 2>&1 || true
  docker volume create "$DATA" >/dev/null
  docker run --rm -v "$SEED":/from -v "$DATA":/to alpine cp -a /from/. /to/
  docker run -d --name "$SERVER" --network "$NETWORK" --cpus "$cpus" --memory "$memory" -v "$DATA":/data "$IMAGE" >/dev/null
  wait_for_server
  for users in $LEVELS; do
    sample >"$RESULTS/.samples" &
    sampler=$!
    report=$(load run --url "http://$SERVER:8080" --accounts accounts.json --users "$users" \
      --duration "$DURATION" --message-every "$MESSAGE_EVERY" --page-every "$PAGE_EVERY" --ramp 10)
    kill "$sampler" 2>/dev/null || true
    wait "$sampler" 2>/dev/null || true
    usage=$(awk '{ cpu += $1; if ($1 > cpu_max) cpu_max = $1; if ($2 > mem_max) mem_max = $2; n++ }
      END { if (n == 0) n = 1; printf "{\"cpu_avg_pct\":%.1f,\"cpu_max_pct\":%.1f,\"memory_max_mib\":%.1f}", cpu / n, cpu_max, mem_max }' \
      "$RESULTS/.samples")
    line=$(printf '%s' "$report" | sed "s/}\$/,\"cpus\":\"$cpus\",\"memory\":\"$memory\",\"server\":$usage}/")
    echo "$line" >>"$out"
    echo "$line" | awk -v users="$users" '{ print "  " users " people: " $0 }' | cut -c1-240
    passed=$(printf '%s' "$line" | awk -v limit="$LIMIT_MS" '
      { ok = 1
        if (match($0, /"delivery_ms":\{[^}]*"p95":[0-9.]+/)) { s = substr($0, RSTART, RLENGTH); sub(/.*"p95":/, "", s); if (s + 0 > limit) ok = 0 }
        if (match($0, /"post_ms":\{[^}]*"p95":[0-9.]+/)) { s = substr($0, RSTART, RLENGTH); sub(/.*"p95":/, "", s); if (s + 0 > limit) ok = 0 }
        if (match($0, /"delivered_ratio":[0-9.]+/)) { s = substr($0, RSTART, RLENGTH); sub(/.*:/, "", s); if (s + 0 < 0.99) ok = 0 }
        if ($0 ~ /"(post_errors|page_errors|socket_errors|resyncs)":[1-9]/) ok = 0
        print ok }')
    if [ "$passed" != 1 ]; then
      echo "  stopped: $users people is past this size's limit"
      break
    fi
  done
  docker rm -f "$SERVER" >/dev/null
  docker volume rm -f "$DATA" >/dev/null
  rm -f "$RESULTS/.samples"
done
