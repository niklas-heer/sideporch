+++
title = "How big a server"
description = "How many people a small server serves, how that was measured, and how to measure it yourself."
weight = 9
+++

Short answer: a lot more than a small group needs. On a server with half a CPU and 512 MB of memory, Sideporch kept messages arriving within a fraction of a second for **4,800 people online at once**, each of them posting every two minutes. Most teams are nowhere near that.

| Server | People online at once, still fast | Memory used then |
| --- | ---: | ---: |
| ½ CPU, 512 MB | 4,800 | 94 MB |
| 1 CPU, 1 GB | 9,600 | 193 MB |
| 2 CPUs, 2 GB | 12,800 or more (the most tested) | 255 MB |

**"Fast"** means 95% of messages are posted, and reach everyone looking at the channel, within one second. Nothing may fail, and nobody may fall behind.

## What to run

These numbers came from a fast machine, and real servers' CPUs are often slower. To leave room, plan with a quarter of them:

- **Up to 1,000 people online at once:** the smallest VPS you can find, a Raspberry Pi 4, or any home server. Half a CPU and 512 MB are plenty.
- **Up to 2,500:** 1 CPU and 1 GB.
- **Up to 5,000:** 2 CPUs and 2 GB.
- **More than that:** it will likely work on bigger machines, but it wasn't tested. Sideporch runs on one server with SQLite, which is not built for organisations of tens of thousands.

People online at once are usually a fraction of everyone with an account. Accounts themselves cost almost nothing: the tests had 12,800.

[Reading aloud and dictation](@/docs/community/speech-models.md) are the exception: while in use, their models take 0.6 to 3.5 GB of memory, and they run one at a time. Small servers are better off with Whisper tiny, or no speech models.

Signing in is deliberately slow work. Passwords are checked with Argon2, about 6 sign-ins a second on half a CPU. Sessions last 30 days, so this only matters if hundreds of people sign in at the same moment.

Disk grows with what people post and upload; text is small, and files take their own size.

## How it was measured

The load generator in [`tools/loadtest/`](https://github.com/niklas-heer/sideporch/tree/main/tools/loadtest) simulates people the way browsers use Sideporch:

- Each person keeps a live connection open.
- Each person looks at one of 10 channels, spread evenly.
- Each person posts a message there every two minutes on average (random intervals), and reloads the channel page every five minutes.
- It measures how long each post takes, how long until everyone else looking at that channel has the message, and how long pages take.

Posting every two minutes is far busier than most teams: 30 messages an hour, per person. With 10 channels, every channel has hundreds or thousands of people looking at it. Both make the test harsher than real use.

Each server size is a container with those CPU and memory limits, running the same image people install. It ran in [Colima](https://github.com/abiosoft/colima) on an Apple silicon Mac, next to the load generator, and started from a database with 12,800 accounts. Each step ran for a minute after everyone had connected; server CPU and memory were sampled every two seconds.

To run it yourself:

```sh
# Build the load generator for Linux, once:
docker run --rm -v "$PWD/tools/loadtest":/src -w /src rust:1-slim-bookworm \
  cargo build --release --target-dir /src/target-linux
# Then pick sizes (CPUs:memory) and numbers of people:
tools/loadtest/run.sh "0.5:512m 1:1g" "100 400 1600 3200"
tools/loadtest/report.sh
```

## What limits it

For most of the range, the limit is CPU: fanning messages out to everyone who sees them. Memory stays small, at about 20 KB per connected person plus a few megabytes.

Version 0.3.0 did much worse:

| Server | 0.3.0 | Now |
| --- | ---: | ---: |
| ½ CPU, 512 MB | 1,200 | 4,800 |
| 1 CPU, 1 GB | 1,600 | 9,600 |
| 2 CPUs, 2 GB | 2,400 | 12,800+ |

These load tests found four problems, all fixed now:

- **Every browser got every message in full.** Sideporch 0.3.0 sent every new message, fully rendered, to every connected browser, whether or not it showed that channel. The work grew with the square of the people online. Browsers now say which channel they show; everyone else gets a short "something new" notice, once until they read it.
- **Rendering rebuilt everyone's usernames.** Every message and page rebuilt a list of all usernames and custom emoji. It is now kept until accounts or emoji change.
- **Mentions checked every account.** Finding who a message mentions scanned every account; it now looks up only the names in the message.
- **WebSocket buffers took most of the memory.** Each connection reserved 128 KB buffers, about 145 KB per person online. That filled half a gigabyte at 4,800 people. They are now sized for the small messages Sideporch sends.

## All results

Latencies are the 95th and 99th percentiles over the minute. "Seen" is from posting until the message reached everyone else looking at the channel. Server CPU is the average share of one core. The 0.3.0 runs started from 3,200 accounts, the others from 12,800.

### 0.3.0: ½ CPU, 512 MB

| People online | Messages/s | Post p95 | Seen p95 | Seen p99 | Page p95 | Server CPU (avg) | Memory (max) | Result |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 50 | 0.4 | 10 ms | 16 ms | 19 ms | 10 ms | 1 % | 13 MiB | ok |
| 100 | 0.7 | 10 ms | 23 ms | 25 ms | 9 ms | 2 % | 23 MiB | ok |
| 200 | 1.9 | 16 ms | 35 ms | 70 ms | 9 ms | 6 % | 40 MiB | ok |
| 400 | 3.4 | 87 ms | 113 ms | 184 ms | 72 ms | 15 % | 74 MiB | ok |
| 800 | 6.9 | 203 ms | 280 ms | 337 ms | 213 ms | 33 % | 137 MiB | ok |
| 1200 | 9.9 | 542 ms | 762 ms | 906 ms | 524 ms | 44 % | 213 MiB | ok |
| 1600 | 13.4 | 2.2 s | 2.9 s | 3.3 s | 2.3 s | 46 % | 289 MiB | too slow |

### 0.3.0: 1 CPU, 1 GB

| People online | Messages/s | Post p95 | Seen p95 | Seen p99 | Page p95 | Server CPU (avg) | Memory (max) | Result |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 50 | 0.4 | 11 ms | 18 ms | 21 ms | 9 ms | 1 % | 13 MiB | ok |
| 100 | 0.8 | 13 ms | 25 ms | 30 ms | 12 ms | 2 % | 21 MiB | ok |
| 200 | 1.7 | 11 ms | 36 ms | 40 ms | 8 ms | 6 % | 42 MiB | ok |
| 400 | 3.6 | 40 ms | 58 ms | 70 ms | 32 ms | 17 % | 71 MiB | ok |
| 800 | 6.6 | 100 ms | 128 ms | 190 ms | 67 ms | 32 % | 139 MiB | ok |
| 1200 | 9.8 | 175 ms | 235 ms | 308 ms | 168 ms | 62 % | 200 MiB | ok |
| 1600 | 13 | 277 ms | 385 ms | 460 ms | 265 ms | 79 % | 262 MiB | ok |
| 2400 | 19.9 | 1.8 s | 2.5 s | 3 s | 1.7 s | 87 % | 400 MiB | too slow |

### 0.3.0: 2 CPUs, 2 GB

| People online | Messages/s | Post p95 | Seen p95 | Seen p99 | Page p95 | Server CPU (avg) | Memory (max) | Result |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 50 | 0.5 | 12 ms | 15 ms | 17 ms | 8 ms | 1 % | 12 MiB | ok |
| 100 | 1 | 10 ms | 17 ms | 19 ms | 8 ms | 3 % | 21 MiB | ok |
| 200 | 1.6 | 11 ms | 24 ms | 27 ms | 10 ms | 6 % | 37 MiB | ok |
| 400 | 3.7 | 18 ms | 38 ms | 47 ms | 16 ms | 20 % | 69 MiB | ok |
| 800 | 6.4 | 46 ms | 63 ms | 81 ms | 31 ms | 44 % | 134 MiB | ok |
| 1200 | 10.3 | 62 ms | 84 ms | 123 ms | 65 ms | 80 % | 199 MiB | ok |
| 1600 | 13.1 | 106 ms | 147 ms | 194 ms | 108 ms | 106 % | 263 MiB | ok |
| 2400 | 21.1 | 259 ms | 350 ms | 418 ms | 256 ms | 162 % | 394 MiB | ok |
| 3200 | 25.6 | 1.3 s | 1.7 s | 2.3 s | 1.3 s | 163 % | 528 MiB | too slow |

### Now: ½ CPU, 512 MB

| People online | Messages/s | Post p95 | Seen p95 | Seen p99 | Page p95 | Server CPU (avg) | Memory (max) | Result |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 800 | 6.5 | 18 ms | 22 ms | 31 ms | 13 ms | 11 % | 19 MiB | ok |
| 1600 | 13.2 | 28 ms | 36 ms | 74 ms | 23 ms | 25 % | 36 MiB | ok |
| 3200 | 26.4 | 72 ms | 97 ms | 412 ms | 75 ms | 38 % | 65 MiB | ok |
| 4800 | 38.8 | 123 ms | 160 ms | 262 ms | 96 ms | 43 % | 94 MiB | ok |
| 6400 | 52.9 | 10.5 s | 9.4 s | 12.3 s | 8 s | 50 % | 319 MiB | too slow |

### Now: 1 CPU, 1 GB

| People online | Messages/s | Post p95 | Seen p95 | Seen p99 | Page p95 | Server CPU (avg) | Memory (max) | Result |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 800 | 6.1 | 15 ms | 19 ms | 27 ms | 10 ms | 11 % | 19 MiB | ok |
| 1600 | 13.8 | 21 ms | 26 ms | 35 ms | 15 ms | 24 % | 34 MiB | ok |
| 3200 | 27.4 | 20 ms | 27 ms | 43 ms | 15 ms | 35 % | 61 MiB | ok |
| 4800 | 40.4 | 20 ms | 26 ms | 36 ms | 17 ms | 46 % | 88 MiB | ok |
| 6400 | 53.8 | 58 ms | 71 ms | 176 ms | 45 ms | 74 % | 125 MiB | ok |
| 9600 | 80.8 | 380 ms | 471 ms | 600 ms | 272 ms | 88 % | 193 MiB | ok |
| 12800 | 105.7 | 8.2 s | 10 s | 11.5 s | 6.6 s | 95 % | 544 MiB | too slow |

### Now: 2 CPUs, 2 GB

| People online | Messages/s | Post p95 | Seen p95 | Seen p99 | Page p95 | Server CPU (avg) | Memory (max) | Result |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 800 | 7.1 | 18 ms | 20 ms | 26 ms | 13 ms | 12 % | 20 MiB | ok |
| 1600 | 13.8 | 18 ms | 21 ms | 29 ms | 15 ms | 29 % | 33 MiB | ok |
| 3200 | 26.9 | 19 ms | 24 ms | 37 ms | 18 ms | 51 % | 61 MiB | ok |
| 4800 | 39.3 | 18 ms | 22 ms | 43 ms | 13 ms | 61 % | 87 MiB | ok |
| 6400 | 52.2 | 23 ms | 29 ms | 54 ms | 17 ms | 86 % | 119 MiB | ok |
| 9600 | 79.5 | 79 ms | 94 ms | 345 ms | 58 ms | 154 % | 197 MiB | ok |
| 12800 | 105.8 | 285 ms | 340 ms | 430 ms | 204 ms | 168 % | 255 MiB | ok |

