+++
schema_version = 1
id = "01M3PS7W101XYK31CTY81FKMXH"
title = "Remind admins of new releases and install only releases signed with Sideporch's key"
date = "2026-09-29"
status = "accepted"
tags = ["updates", "security", "distribution"]
supersedes = []
superseded_by = []
depends_on = ["01M3J7NEGN00V51QXJAHX99QWB"]
related_to = []
+++
## Decision

Sideporch tells admins about new releases and can install them itself, only when the release is signed with Sideporch's own key.

- **Checking**: every six hours the server asks GitHub's releases API which releases exist. It sends nothing about the server. Admins turn it off under Admin → Updates; `--update-check false` (`SIDEPORCH_UPDATE_CHECK`) keeps Sideporch from contacting GitHub at all.
- **Security releases** are those whose notes have a Security section, which `fix(security)` commits produce through `cliff.toml`.
- **Reminders grow**: a new release is listed under Admin → Updates; after 14 days admins get a sidebar reminder they can hide for a week; after 60 days, or at once for a security release, it comes back daily. Only admins see it.
- **Signing**: the release workflow signs `SHA256SUMS` with minisign (prehashed Ed25519), with the version in the trusted comment. The public key is built into Sideporch (`src/updates/signature.rs`), published at `website/static/sideporch.pub`, and checked against the signature before a release is published. Sideporch installs an update only when the signature, the version in it, the archive's SHA-256 and the new program's `--version` all match.
- **Installing**: `sideporch update` replaces the program for archive installs and keeps the old one as `sideporch.previous`. When the running server may write its program's directory, admins can press Update now, and choose what installs by itself: nothing, security fixes (the default) or every release (at night). The server then replaces itself with `exec`, keeping its process, so service managers don't notice. Homebrew, Nix and container installs are never replaced; Sideporch shows their own update command.
- **Key custody**: the secret key is the GitHub Actions secret `SIDEPORCH_RELEASE_SIGNING_KEY`, with a backup outside GitHub. Releases before 0.5.0 are unsigned and can't be installed this way.

## Context

On 2026-09-29 Niklas asked to recommend the `curl` install for servers, to make self-updating possible, and to check regularly for new versions so that "the longer you stay out of date, the more annoying it gets", with security fixes in mind.

- `SHA256SUMS` from the same GitHub release only proves a download is intact, not that it comes from the project: whoever controls the release could change both. A key held only by the release workflow, and trusted by every installed server, closes that gap.
- minisign was chosen over GPG and Sigstore: one small key, a format the `minisign-verify` crate reads without dependencies, and a CLI people can use to check downloads themselves. Sigstore would tie verification to online transparency logs and OIDC identities, which a self-hosted server can't always reach.
- GitHub's releases API is rate-limited per IP (60 an hour without a token); four checks a day stay far below it. A release feed on sideporch.app was considered, but would need publishing steps of its own.
- Letting the service replace its own program is a trade-off: simpler to keep current, but a compromised process could change it too. It's therefore only possible where admins let the service user write the program's directory; the documented systemd setup keeps the program read-only and uses `sudo sideporch update`.
- Replacing the process with `exec` keeps its PID, so systemd, Docker restart policies and monitoring see no crash; browsers reconnect by themselves.

## Consequences

- Losing the signing key means installed servers can't update themselves until an admin installs a release by hand; a new key needs a release signed with the old one, carrying the new key. Keep the backup.
- Security fixes must be committed as `fix(security): …`, or servers won't treat them as urgent.
- Tests sign fake releases with throwaway keys (`tests/updates.rs`); `tests/fixtures/release/` holds a fixture signed with the real key, checking that the built-in key and minisign's format still match.
- `install.sh` checks the signature too when minisign is installed.
