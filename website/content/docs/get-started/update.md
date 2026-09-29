+++
title = "Update"
description = "Install a new version and restart; the database upgrades itself and keeps a copy first."
weight = 5
+++

Updating Sideporch means installing the new version and restarting it. That's all, even when you skip several versions.

## Install the new version

| Installed with | Update with |
| --- | --- |
| The install script | Run the script again, then `sudo systemctl restart sideporch`. |
| Docker | `docker pull ghcr.io/niklas-heer/sideporch` and recreate the container; with Compose, `docker compose pull && docker compose up -d`. |
| Homebrew | `brew upgrade sideporch`, then `brew services restart sideporch`. |
| NixOS | Update the flake input (`nix flake update sideporch`) and rebuild. |

Read the [release notes](https://github.com/niklas-heer/sideporch/releases) first: anything you need to do before upgrading is listed there.

## What happens on start

On start, Sideporch applies every database change between the version you ran and the new one, in order, each in its own transaction. An interrupted upgrade picks up where it stopped.

Before it changes the database, Sideporch keeps a copy of it in `upgrade-backups/` in the data directory. It keeps the newest three copies.

## Go back to the version before

Stop Sideporch, restore the copy, and start the version you ran before:

```sh
sideporch restore /var/lib/sideporch/upgrade-backups/sideporch-schema17-20261001-080000.db \
  --data /var/lib/sideporch --force
```

An older version refuses to start on a database a newer one has already upgraded, and names the version that did, rather than guessing at tables it doesn't know.

{% <note kind="warning"> %}
The copy holds the database as it was before the upgrade. Messages written since then are only in the current database, so take a [backup](@/docs/community/backups.md) of it first if you need them.
{% </note> %}
