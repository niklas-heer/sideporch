# Future ideas

Product ideas worth keeping in mind. Agreement on a feature does not establish its technical design or mean it has been implemented. Each says what exists today, so a later design starts from there instead of from scratch.

Notification controls, group direct messages, thread following, OpenID Connect sign-in, retry-safe sends and reconnects, the bot API, the automation store and typing across connected servers have been built; see the decision records and the documentation.

**Email catch-up is deferred.** A recipient's email address is not a delivery service: Sideporch already supports an optional admin-configured SMTP relay for sign-in mail (`src/mail.rs`). A future catch-up feature could reuse that relay without requiring the operator to run a mail server.

**Calls and screen sharing are a larger task.** Consider them separately; no calling architecture has been selected.

Competitive comparisons should include small self-hosted chats, especially single-binary projects such as [ClickClack](https://github.com/openclaw/clickclack), as well as established products. The single-binary packaging baseline is already recorded in the decision "Ship Sideporch as a single self-contained Rust binary".

Other useful comparison sources: [Chatto](https://github.com/chattocorp/chatto/tree/main/apps/docs-website/src/content/docs) for a single-binary chat with optional LiveKit for calls, [ForumChat](https://github.com/atvirokodosprendimai/forumchat) for forwarding and promoting chat into durable discussions, and [Campfire](https://github.com/basecamp/once-campfire) for a simple self-contained container deployment. Campfire is a single-container comparison, not a single-binary one. Feature documentation alone does not establish reliability or maturity.

## Plugins

**Plugins** that change Sideporch itself (new pages, new message kinds) are a larger step than the automation store, which only carries sandboxed automations. They'd need a stable extension API beyond the Lua sandbox.

The store has no reviews or verified authors yet. They would be worth adding if it grows.

## The bot API

The API covers public channels only, which keeps the existing public-only automation boundary. Private-channel and DM access would need an explicit design. There is no SDK or CLI for it.
