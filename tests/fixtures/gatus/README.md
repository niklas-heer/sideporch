# Gatus webhook payloads

These are the exact request bodies Gatus sends to a Slack-compatible
webhook. They were produced by Gatus's own payload builders, not written by
hand, so tests exercise what Gatus really sends.

- Source: [TwiN/gatus](https://github.com/TwiN/gatus) at commit
  `ae605f291928ed9b3bdbe60ae5528cc57631433b` (2026-09-23).
- `slack-*.json`: the `slack` alert provider.
- `mattermost-*.json`: the `mattermost` provider, which adds `channel`,
  `username` and `icon_url`, and sends `"fields": null` when there are no
  condition results.

Both providers send `Content-Type: application/json` and treat any response
status of 400 or above as a failed alert.

## Regenerating

Copy `capture_slack_test.go.txt` to `alerting/provider/slack/capture_test.go`
and `capture_mattermost_test.go.txt` to
`alerting/provider/mattermost/capture_test.go` in a Gatus checkout, then run:

```sh
mkdir -p /tmp/gatus-fixtures
go test ./alerting/provider/slack/ ./alerting/provider/mattermost/ -run TestCaptureFixtures -count=1
```

The files appear in `/tmp/gatus-fixtures/`.
