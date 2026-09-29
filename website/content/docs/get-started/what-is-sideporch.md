+++
title = "What Sideporch is"
description = "A small, self-hosted team chat in one program: what it's good at, and what it deliberately isn't."
weight = 1
+++

Sideporch is team chat you run yourself: channels, threads and direct messages for a team, a club, a family or a public community, with the things people miss from Slack. It is one program and one data directory, on a server you control.

{{<shot name="channel" alt="A Sideporch channel with a thread open on the right: messages with reactions, a checklist, and a ranked poll showing which restaurant leads after three rounds." caption="A channel with a thread open, and a ranked poll finding where the team celebrates." dark={true} />}}

## Why Sideporch

- **Yours, and simple to run.** One program and one data directory. No database server, no mail server required, nothing else to install. It runs on a small VPS, a home server or an ARM board.
- **Familiar.** Channels, threads, mentions, reactions, search and notifications work the way people know from Slack, and you can [bring your Slack history along](@/docs/community/connect/move-from-slack.md).
- **Easy to back up and move.** Everything lives in one directory. [Download a complete backup](@/docs/community/server/backups.md) from the browser, or have Sideporch write one every day.
- **Connects with other servers.** Like Slack Connect, [two Sideporch servers can connect](@/docs/community/connect/other-servers.md) to share channels and let people write to each other.
- **Works with your tools.** Monitors, CI and bots post through [Slack-compatible webhooks](@/docs/integrations/incoming-webhooks.md), so tools like Gatus and Grafana work unchanged. [Automations in Lua](@/docs/integrations/automations/_index.md) do the rest.

{% <note> %}
Sideporch is young. It works end to end, but expect rough edges, and read the [release notes](https://github.com/niklas-heer/sideporch/releases) before upgrading, until 1.0.
{% </note> %}

## Good to know

- **It's for teams, not enterprises.** Sideporch runs on one server with SQLite. That is plenty for a team, a club, a company of a few thousand or a family, but it isn't built for organisations of tens of thousands. [How big a server](@/docs/community/server/server-size.md) has the numbers.
- **The server can read everything.** Messages are not end-to-end encrypted. Whoever runs the server, and admins through backups, can read all of them. Use HTTPS.
- **No calls, no email notifications.** There are no voice or video calls, and notifications are push notifications, not email. Email, when set up, is only for signing in.
- **No group direct messages.** Make a private channel instead.
- **Automations only see public channels**, never private channels or direct messages.

## Where to go next

- [Try it](@/docs/get-started/try-it.md) on your own computer in a minute.
- [Install it on a server](@/docs/get-started/install.md) to use it with other people.
