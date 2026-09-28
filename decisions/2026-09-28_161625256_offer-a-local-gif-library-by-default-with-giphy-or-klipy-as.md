+++
schema_version = 1
id = "01M3MCV858SVVQE3G3CQWCVDM7"
title = "Offer a local GIF library by default, with GIPHY or KLIPY as options"
date = "2026-09-28"
status = "accepted"
tags = ["messages", "integrations"]
supersedes = ["01M3M94K5SZGKP1431KYHG80RG"]
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

GIFs come from a team library by default. An admin can switch the source under Admin → GIFs to GIPHY or KLIPY instead; each needs its own API key, stored encrypted like secrets, and a content rating applies to both.

- **Local library** (default): anyone signed in adds a GIF, WebP, PNG or JPEG (up to 10 MB) with a title and tags at `/gifs/library`. Files are stored like other uploads. The picker searches titles and tags, most used first. The person who added a GIF, or an admin, can remove it, which also removes it from old messages.
- **GIPHY**: unchanged from the superseded decision. Sideporch searches on people's behalf, so the key stays on the server, and fetches a posted GIF again by id. Media stays on GIPHY's URLs with "Powered by GIPHY".
- **KLIPY**: KLIPY's integration requirements say API requests and media loads must come from people's browsers unless KLIPY approves otherwise. So the picker calls KLIPY's Tenor-compatible API (`api.klipy.com/v2`) from the page, with the admin's key, and the Content Security Policy allows that host in `connect-src`. Posting sends the media URL; the server accepts it only over HTTPS from `klipy.com` or its subdomains. The picker and messages credit KLIPY.
- A service chosen without a saved key falls back to the library. Switching sources keeps saved keys.

## Context

On 2026-09-28 Niklas asked for a GIPHY option and a KLIPY option, with a local library as the default, because a library needs no outside account and is the simplest to offer in a self-hosted tool.

- Tenor closed its API on 2026-06-30.
- GIPHY forbids caching media and altering URLs, so its GIFs cannot be imported into the library.
- KLIPY's terms have the same effect and additionally require browser-side requests.

## Consequences

- With KLIPY chosen, every signed-in browser receives the KLIPY key. The settings page says so.
- With GIPHY or KLIPY chosen, viewing a GIF reveals the viewer's address to that service. Messages set `referrerpolicy="no-referrer"`.
- Library GIFs count against the data directory's disk space and are backed up with it.
- Old messages keep whichever source they were posted from, so switching sources does not break them.
