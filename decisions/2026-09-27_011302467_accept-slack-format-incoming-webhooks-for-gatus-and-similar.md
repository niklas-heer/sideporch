+++
schema_version = 1
id = "01M3G6RCR3CD9FBE9MT15D4RRX"
title = "Accept Slack-format incoming webhooks for Gatus and similar tools"
date = "2026-09-27"
status = "accepted"
tags = ["integrations", "webhooks", "compatibility"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch accepts incoming webhooks in Slack's format, so monitors and other tools that already post to Slack can post to a Sideporch channel unchanged. [Gatus](https://gatus.io/) is the first target; similar systems such as Grafana and CI services are expected to work through the same endpoint.

For Gatus, the endpoint must at least:

- accept a JSON `POST` at a per-channel webhook URL;
- render `text` and `attachments`, each with `title`, `text`, `short`, `color`, and optional `fields` of `title`, `value`, and `short`;
- honour the optional `channel`, `username`, and `icon_url` fields that Gatus's `mattermost` provider adds;
- answer with a status below 400 on success, because Gatus treats any status of 400 or above as a failed alert.

## Context

On 2026-09-27 Niklas asked for Sideporch to be compatible with Gatus "or similar systems". Gatus already has alert providers for Slack, Mattermost, Rocket.Chat, and a generic `custom` webhook ([provider list](https://github.com/TwiN/gatus/tree/master/alerting/provider)). Its [`slack` provider](https://github.com/TwiN/gatus/blob/master/alerting/provider/slack/slack.go) posts the payload above to a configured `webhook-url`. Mattermost and Rocket.Chat accept the same Slack-style format, which is why so many tools support it.

Alternatives considered:

- **A dedicated Gatus provider upstream.** This needs an upstream contribution, and it helps only Gatus.
- **Gatus's `custom` provider with a Sideporch-specific format.** This works, but every user must write a request template, and other tools gain nothing.

## Consequences

- Sideporch has to keep a stable subset of Slack's webhook schema. Unknown fields should be ignored rather than rejected.
- Compatibility needs tests against payloads captured from Gatus's `slack` and `mattermost` providers.
- Outgoing webhooks and a native bot API are separate decisions.
