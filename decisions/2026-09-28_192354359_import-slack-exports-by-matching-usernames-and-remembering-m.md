+++
schema_version = 1
id = "01M3MQJHKQ0BXZJAF2AN12BNPX"
title = "Import Slack exports by matching usernames and remembering message origins"
date = "2026-09-28"
status = "accepted"
tags = ["import", "slack"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Admin → Import reads a Slack workspace export ZIP. People are matched to existing accounts by username; the rest are created with a password nobody knows, so admins hand out reset links, and people deleted in Slack are created deactivated. Channels are matched by name. Each imported message stores its Slack origin in `import_id` (unique), so importing again adds only what is new. Messages keep Slack formatting (`slack_format`), mentions become @usernames, and file attachments are named, since Slack exports link files behind Slack's login.

## Context

On 2026-09-28 Niklas agreed to Slack import as the biggest step for teams moving over. Exports have `users.json`, `channels.json`, `groups.json`, `dms.json`, `mpims.json` and a folder per conversation with a JSON file per day.

## Consequences

- Group conversations become private channels, as Sideporch has no group DMs.
- Imported history starts out read and sends no notifications.
- The import runs in one transaction on the main connection, so the server pauses writes while a large export imports.
