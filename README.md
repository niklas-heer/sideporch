# Sideporch

Sideporch is a small, self-hosted team chat: channels, direct messages, and threads for a team, a club, or a family. It ships as one binary you copy to a server and run.

> **Status:** early planning. There is no code yet. The design is still being worked out.

## Goals

- **One binary.** The server, realtime WebSocket connection, and web interface ship as a single self-contained Rust executable. No external database, message broker, cache, or container runtime.
- **Easy to run and move.** Your data should be easy to back up, copy to another server, and replicate.
- **Built for small groups.** Invite links instead of an email server, and sensible defaults instead of configuration.
- **Works with your existing tools.** Monitors, CI systems, and bots post into channels through webhooks. Services that already speak Slack's incoming-webhook format, such as [Gatus](https://gatus.io/) and Grafana, work unchanged.

## Open questions

- Storage engine and on-disk format.
- Web frontend approach: a Rust/WebAssembly framework such as Leptos, or server-rendered HTML with a little JavaScript.
- Scope of the first release.

## Decisions

Lasting choices are recorded in [decisions/](decisions/) using [vrdx](https://github.com/niklas-heer/vrdx).

## License

[MIT](LICENSE)
