+++
title = "GIFs, link previews and the system page"
description = "Choose where GIFs come from, turn link previews on or off, and watch the server's health."
weight = 9
+++

## GIFs

Under **Admin → GIFs**, choose where the GIF picker finds GIFs:

- **The team's own library** (the default): GIFs people upload to the **GIF library** in their account menu. Nothing leaves your server.
- **GIPHY**: Sideporch searches GIPHY for people, so the API key stays on the server. The GIFs themselves load from GIPHY's servers, as its terms require.
- **KLIPY**: people's browsers search KLIPY directly, as its terms require, so they can see the key.

Keys are stored encrypted in the database.

## Link previews

Sideporch fetches a preview (title, description, picture) for links in messages, from the server. Requests to private and local addresses are refused, so a link can't make the server probe your own network. Admins turn previews off under **Admin → Messages**, where they also [limit editing](@/docs/using/chatting.md#edit-delete-and-pin) to a while after sending.

## The system page

**Admin → System** shows how the server is doing: CPU and memory over the last minutes, the sizes of the database and files, free disk space, and activity. Use it to see whether the server still fits; [How big a server](@/docs/community/server-size.md) has the numbers.
