+++
title = "Try it"
description = "Run Sideporch on your own computer in a minute, with Docker or Homebrew."
weight = 2
+++

The quickest way to see Sideporch is to run it on your own computer. Nothing you do here is hard to undo: all data stays in one place you can delete afterwards.

## With Docker

```sh
docker run -d -p 8080:8080 -v sideporch:/data ghcr.io/niklas-heer/sideporch
```

Then open <http://localhost:8080>. Your data lives in the `sideporch` volume; `docker volume rm sideporch` removes it after you've stopped the container.

## With Homebrew

On macOS and Linux:

```sh
brew install niklas-heer/tap/sideporch
sideporch
```

Then open <http://127.0.0.1:8080>. Sideporch keeps its data in `sideporch-data` in the directory you started it from.

## Your first minutes

1. **Create the admin account.** The first person to open Sideporch creates it. That's you.
2. **Invite someone.** Open **People** in the sidebar and create an invite link. Anyone with the link can join; nobody needs an email address.
3. **Look around.** Start a channel, reply in a thread, add a poll with `/poll Lunch? | Pizza | Tacos`, or press <kbd>⌘</kbd> <kbd>K</kbd> (<kbd>Ctrl</kbd> <kbd>K</kbd>) to jump anywhere.

{% <note kind="tip"> %}
Push notifications and installing Sideporch as an app on phones need HTTPS, and passkeys need a name like `localhost` rather than an IP address. They work once Sideporch runs on a server with its own address.
{% </note> %}

To keep it and use it with other people, [install it on a server](@/docs/get-started/install.md).
