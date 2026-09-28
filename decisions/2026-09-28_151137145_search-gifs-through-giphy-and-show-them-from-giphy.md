+++
schema_version = 1
id = "01M3M94K5SZGKP1431KYHG80RG"
title = "Search GIFs through GIPHY and show them from GIPHY"
date = "2026-09-28"
status = "superseded"
tags = ["messages", "integrations"]
supersedes = []
superseded_by = ["01M3MCV858SVVQE3G3CQWCVDM7"]
depends_on = []
related_to = []
+++
## Decision

GIF search uses GIPHY. An admin enables it under Admin → GIFs with a GIPHY API key, stored encrypted like secrets, and a content rating.

- The composer's GIF button searches through Sideporch (`/gifs`), so the key never reaches browsers. An empty search shows trending GIFs.
- Posting sends only the GIF's id. Sideporch fetches the GIF from GIPHY again and stores its title, rendition URL and size on the message, so a message can only show GIPHY media, not an arbitrary URL.
- GIFs are shown from GIPHY's own URLs, unmodified, and the picker and messages credit GIPHY, as GIPHY's API terms require. Media is not downloaded or cached.

## Context

On 2026-09-28 Niklas asked for GIFs, mentioning Tenor or GIPHY.

- **Tenor** closed its public API on 2026-06-30 (new keys stopped in January 2026), so it is not an option.
- **GIPHY's** API terms forbid caching media without approval, require "Powered by GIPHY" attribution, and forbid altering media URLs. That rules out storing GIFs as uploads.
- **KLIPY**, which Discord moved to, is a possible second provider; its documentation could not be checked when this was decided. The settings keep a provider field so it can be added.

## Consequences

- Viewing a GIF makes the browser contact GIPHY's media servers, which reveals the viewer's address to GIPHY. Messages set `referrerpolicy="no-referrer"`.
- If GIPHY changes a URL or removes a GIF, old messages show a broken image.
