+++
schema_version = 1
id = "01M3J7NEGN00V51QXJAHX99QWB"
title = "Ship static musl binaries, a scratch image, and verified installers"
date = "2026-09-27"
status = "accepted"
tags = ["packaging", "distribution", "ci"]
supersedes = []
superseded_by = []
depends_on = ["01M3G6RCQKT16B893YJKV7YY7K"]
related_to = ["01M3J6PYX6SHYR5CPG4FMNEG69"]
+++
## Decision

Sideporch releases are built so that installing it means copying one file:

- **Linux binaries are fully static.** They target `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`, cross-compiled with [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) and zig, which link musl in. They need no libc or other system libraries. The packaging script refuses a Linux binary that `file` does not report as statically linked.
- **macOS binaries** are built natively for Apple silicon and Intel. macOS does not support fully static executables; they link only Apple's system library.
- **The container image is `FROM scratch`**: the static binary alone, running as user 10001, listening on port 8080, with data in `/data`. It is published to `ghcr.io/niklas-heer/sideporch` for amd64 and arm64: `main` from every change on main, and `X.Y.Z`, `X.Y` and `latest` from release tags.
- **Channels**: GitHub release archives with a `SHA256SUMS` file; `install.sh` for `curl | sh`, which verifies the checksum; the `niklas-heer/tap` Homebrew formula, generated from the release checksums like the other formulas in that tap; and a Nix flake with a package, a development shell, and a NixOS module.
- **CI** follows Niklas's stack: Dagger with the Dang SDK in `.dagger/main.dang`, tool versions from `mise.toml`, and thin GitHub Actions workflows. Native macOS checks run on a macOS runner.

## Context

On 2026-09-27 Niklas asked for a Nix flake, a `curl` install command, a Homebrew formula in his tap, CI for Docker, and "a standalone static binary, so you don't need even libc or anything".

Everything Sideporch links is compatible with this. SQLite and Lua are C, compiled by `cc` through zig. TLS uses rustls with the ring provider and bundled Mozilla roots instead of OpenSSL or the system certificate store. On 2026-09-27 a local build produced statically linked binaries of about 6 MB for both architectures. The ARM64 one ran in a `FROM scratch` image (4.6 MB) and on Alpine through `install.sh`.

Alternatives considered:

- **glibc builds** (as repot ships) need a recent enough glibc on the host and cannot run in an empty image.
- **Building musl natively on Alpine per architecture** needs arm64 runners or slow emulation; zig cross-compiles both from one x86-64 container.
- **Distroless or Alpine base images** add files the binary never uses.

## Consequences

- Cross-compiling the C dependencies depends on zig. Keep zig and cargo-zigbuild pinned in `mise.toml` and test the static build when upgrading either.
- The image has no shell. Debugging happens from outside the container, and a health check must use `/healthz` from the host or orchestrator.
- The Homebrew formula updates itself only after a published release: the tap's hourly workflow renders it from `SHA256SUMS` and opens a pull request to merge.
- Release notes are generated from commits by GitHub. Write a release note file if the history stops explaining a release well enough.
