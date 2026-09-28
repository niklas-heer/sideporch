+++
schema_version = 1
id = "01M3MQJHKZP5T1MHE92F83B7S2"
title = "Fetch link previews on the server through the guarded HTTP client"
date = "2026-09-28"
status = "accepted"
tags = ["messages", "security"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

When a person's message contains a link outside code, the server fetches the first one in the background and stores its title, description, site name and image URL on the message. It uses the automations' HTTP client, whose DNS-level guard refuses private and internal addresses, reads only HTML, follows up to three redirects, and waits at most 8 seconds. Images stay on the linked site. Authors and admins remove a preview, and admins can switch previews off.

## Context

On 2026-09-28 Niklas agreed to link previews. Fetching from the server is the only way to read Open Graph tags across origins, and it must not let a message make the server probe its own network.

## Consequences

- Linked sites see the server's address when a link is posted, and viewers' addresses when their browsers load a preview image (without a referrer).
- Edits fetch the preview again, or clear it when the link is gone.
