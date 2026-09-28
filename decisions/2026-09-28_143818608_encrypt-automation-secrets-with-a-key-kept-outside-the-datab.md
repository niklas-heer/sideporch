+++
schema_version = 1
id = "01M3M77KFGMZ3KFYY4E67SKAM2"
title = "Encrypt automation secrets with a key kept outside the database"
date = "2026-09-28"
status = "accepted"
tags = ["automations", "security"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3M77KF4V6HX4AC84FV21HM3"]
+++
## Decision

Automation secrets, and the AI provider's key, are stored in the database encrypted with AES-256-GCM (RustCrypto's `aes-gcm`, already in the dependency tree for Web Push). Each value is bound to its name as associated data, so ciphertexts cannot be swapped.

The key never enters the database. It comes from `SIDEPORCH_SECRET_KEY`, any string stretched with SHA-256, or from `secret.key` in the data directory, 32 random bytes created on first start with mode 0600. Environment variables named `SIDEPORCH_SECRET_<NAME>` are secrets as well and take precedence over stored values of the same name.

Scripts read secrets with `sideporch.secret("NAME")`. The interface and MCP show names and sources only; values are write-only. Secret values of four or more characters are replaced with `[secret NAME]` in run logs, test results and command answers.

## Context

On 2026-09-28 Niklas asked to "store environment variables securely in the environment and in our SQLite file", for automations that call external APIs.

- **Plain values in SQLite** would put tokens into every database backup and replica, such as Litestream copies.
- **Environment variables only** are the twelve-factor answer, but changing one means restarting the server, and admins could not add a token from the browser.
- **An external secret manager** (Vault, 1Password) would contradict the single-binary, no-dependencies goal. Admins who use one can inject values through `SIDEPORCH_SECRET_*`.

## Consequences

- Backups must include `secret.key` or the `SIDEPORCH_SECRET_KEY` value, or stored secrets cannot be decrypted. Losing the key loses the secrets, not the rest of the data.
- A script can still send a secret anywhere it can reach, so admins decide which scripts run. Redaction protects logs, not intent.
