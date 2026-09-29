# Future ideas

Agreed product priorities and ideas worth keeping in mind. Agreement on a feature does not establish its technical design or mean it has been implemented. The additional ideas remain proposals. Each says what exists today, so a later design starts from there instead of from scratch.

## An agreed package to design

On 2026-09-29, Niklas agreed with the competitive-gap recommendations and suggested packaging the following together:

- **Notification controls**: temporary pauses and recurring quiet hours. Today, channels can be muted and devices can enable push, but there is no personal notification schedule.
- **Group direct messages**: start a private conversation with several people without creating and naming a private channel. Today, direct messages are between two people; groups use private channels.
- **Explicit thread following**: follow without posting, unfollow, and prioritize unread followed threads. Today, thread recipients are inferred from the people who wrote in the thread (`src/store.rs`, `recipients`).
- **OpenID Connect sign-in**: an optional generic identity-provider integration. Today, sign-in supports passwords, passkeys, authenticator codes and email links.

**Email catch-up is deferred.** A recipient's email address is not a delivery service: Sideporch already supports an optional admin-configured SMTP relay for sign-in mail (`src/mail.rs`). A future catch-up feature could reuse that relay without requiring the operator to run a mail server.

**Calls and screen sharing are a larger task.** Consider them separately from this package; no calling architecture has been selected.

Competitive comparisons should include small self-hosted chats, especially single-binary projects such as [ClickClack](https://github.com/openclaw/clickclack), as well as established products. The single-binary packaging baseline is already recorded in the decision "Ship Sideporch as a single self-contained Rust binary".

## Additional candidates from smaller chats

These are proposals from the comparison on 2026-09-29, not additions to the agreed package. Sources were read and selected ClickClack implementation paths inspected; the competitors were not run. Recheck their current implementation and Sideporch's behavior before designing these features.

- **Retry-safe sends and smoother reconnects.** ClickClack [documents durable event replay](https://github.com/openclaw/clickclack/blob/main/docs/features/realtime.md) and [idempotent message creation with a client nonce](https://github.com/openclaw/clickclack/blob/main/docs/features/messages.md). Its `realtime_order_test.go` covers late publication across reconnects. Sideporch already reloads after reconnecting and preserves text drafts in session storage (`assets/app.js`); its current send path has no client idempotency key. Investigate preserving the current view during recovery and reconciling a send whose response was lost. Do not treat the existing reload as an absence of recovery, or assume that smoother recovery requires copying ClickClack's event-log architecture.
- **A scoped external chat API and bot identities.** ClickClack [documents service bots and user-owned bots with scoped, revocable tokens](https://github.com/openclaw/clickclack/blob/main/docs/features/bots.md); its `bot_scope.go` checks workspace and DM scope boundaries. Sideporch already has incoming and outgoing webhooks, Lua automations, and admin-authenticated MCP tools that can read recent public-channel messages. It does not offer the same general external chat API or independently scoped bot credentials. Investigate a small API for external tools before committing to an SDK or CLI. Private-channel and DM access would need an explicit design; the current public-only automation boundary remains in force.

Other useful comparison sources: [TensorChat](https://github.com/tensorspace-ai/tensorchat/blob/main/README.md) for group DMs and generic OIDC, [Chatto](https://github.com/chattocorp/chatto/tree/main/apps/docs-website/src/content/docs) for a single-binary chat with optional LiveKit for calls, [ForumChat](https://github.com/atvirokodosprendimai/forumchat) for forwarding and promoting chat into durable discussions, and [Campfire](https://github.com/basecamp/once-campfire) for a simple self-contained container deployment. Campfire is a single-container comparison, not a single-binary one. Feature documentation alone does not establish reliability or maturity.

## An automation store, and plugins

A place on sideporch.app where people publish automations and libraries, and where an admin can browse them and install one with a click.

What exists: automations already travel as versioned JSON files (`src/automations/bundle.rs`, decision "Share automations as versioned JSON files that import switched off"). Those files accept optional `description`, `author`, `license` and `homepage` fields, and readers ignore fields they don't know. Importing always goes through a preview, and imported scripts start switched off.

What a store would need:

- **Importing from a URL**: fetch a file through the guarded HTTP client (`src/automations/http.rs`) and hand it to the existing preview.
- **A catalogue**: a static index on sideporch.app (one JSON file listing entries and their file URLs) would do at first; no server needed beyond the website.
- **Trust**: files signed like releases (`src/updates/signature.rs`), so a server knows an entry wasn't changed on the way. Reviews or verified authors, if the store grows.
- **Updates**: remembering where an automation came from (a new column, for example `automations.origin`) so Sideporch can offer a newer version. Replacing already keeps history and leaves it switched off.
- **Settings instead of editing**: many shared automations need a channel name or a threshold. A declared list of settings in the file, shown as a form, would spare people editing Lua. The sandbox could expose them as `sideporch.setting("name")`.

**Plugins** that change Sideporch itself (new pages, new message kinds) are a larger step. They'd need a stable extension API beyond the Lua sandbox. The store could start with automations, which are sandboxed already, and grow from there.

## Typing across connected servers

Typing shows to people on the same server only. Shared channels on other servers don't see it, because the federation outbox delivers every event in order and retries it, which is wrong for a signal that is stale after a few seconds. A best-effort, unqueued notice between servers would fix that.
