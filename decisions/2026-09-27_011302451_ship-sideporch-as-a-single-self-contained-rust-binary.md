+++
schema_version = 1
id = "01M3G6RCQKT16B893YJKV7YY7K"
title = "Ship Sideporch as a single self-contained Rust binary"
date = "2026-09-27"
status = "accepted"
tags = ["packaging", "architecture"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch ships as one self-contained Rust executable. It contains the HTTP server, the realtime WebSocket connection, and all web interface assets. Running it must not require an external database, message broker, cache, or container runtime. Deploying or moving an instance means copying the binary and its data.

This record covers packaging only. The storage engine, on-disk format, and frontend framework are still open.

## Context

Niklas proposed Sideporch on 2026-09-27 as a simple, open-source Slack alternative that is easy to replicate and run on a server "with no fiddling around". Established self-hosted chat servers need several services: Zulip runs beside Postgres, RabbitMQ, memcached, and Redis. Mattermost's free self-hosted mode now shows only the last 10,000 messages.

Other projects already ship as one binary or one container, including Chatto (Go, embedded NATS, SQLite) and Campfire (Rails and SQLite in one Docker image). So single-binary deployment is expected rather than distinctive. It is still the baseline this project must meet.

Rust was Niklas's stated choice. It produces static binaries, and assets can be compiled into the executable.

## Consequences

- Every dependency must be embeddable, which rules out designs that need a separate database server or broker.
- Web assets are built at compile time and embedded, so changing the frontend needs a rebuild.
- Scaling across several processes is out of scope unless a later decision revisits this record.
