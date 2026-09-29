+++
title = "Update"
description = "Hear about new releases, install them with one command or by themselves, and go back if you need to."
weight = 5
+++

Updating Sideporch means installing the new version and restarting it. That's all, even when you skip several versions: the database upgrades itself on start and keeps a copy first.

## Hearing about new releases

{% <note kind="tip"> %}
New in 0.5.0.
{% </note> %}

Every six hours Sideporch asks GitHub which releases exist. Nothing about your server is sent. Admins see new releases under **Admin → Updates**, and a reminder in the sidebar that grows more insistent the longer the server stays behind:

| When | Admins see |
| --- | --- |
| A new release is out | It's listed under Admin → Updates. |
| It has been out for two weeks | A reminder they can hide for a week. |
| Updates have waited for two months | A reminder that comes back every day. |
| A newer release fixes a security problem | A red reminder, right away, that comes back every day. |

Releases that fix security problems have a **Security** section in their [release notes](https://github.com/niklas-heer/sideporch/releases).

To keep Sideporch from contacting GitHub at all, for example on a server without internet access, start it with `--update-check false` (or `SIDEPORCH_UPDATE_CHECK=false`). Admins can also turn checking off under Admin → Updates.

## Install the new version

| Installed with | Update with |
| --- | --- |
| The install script | `sudo sideporch update`, then `sudo systemctl restart sideporch`. Or **Update now** under Admin → Updates, when [Sideporch may update itself](#let-sideporch-update-itself). |
| Docker | `docker pull ghcr.io/niklas-heer/sideporch` and recreate the container; with Compose, `docker compose pull && docker compose up -d`. |
| Homebrew | `brew upgrade sideporch`, then `brew services restart sideporch`. |
| NixOS | Update the flake input (`nix flake update sideporch`) and rebuild. |

Admin → Updates shows the right way for your server.

Read the release notes first: anything you need to do before upgrading is listed there.

## `sideporch update`

```sh
sideporch update --check    # is there a newer release?
sudo sideporch update       # install it in place of this program
sudo sideporch update --version 0.5.0
```

It downloads the release for your system and installs it only when the release's checksums are signed with Sideporch's release key, which is built into the program, and the download matches them. The program it replaces stays next to it as `sideporch.previous`. Restart Sideporch afterwards.

Releases before 0.5.0 aren't signed, so `sideporch update` installs 0.5.0 and later.

## Let Sideporch update itself

When Sideporch may change the file it runs from, admins can install a release with **Update now**, and choose under Admin → Updates what installs by itself:

- **Nothing**: admins install updates.
- **Security fixes** (the default): releases that fix security problems install as soon as Sideporch hears of them.
- **Every release**: security fixes right away, everything else between 3 and 5 in the morning.

After installing, Sideporch restarts into the new version by itself, in the same process, so systemd, Docker's restart policy and your monitoring don't notice. People online see "reconnecting" for a moment.

With the [systemd setup](@/docs/get-started/run-on-a-server.md#start-it-as-a-service), Sideporch runs as its own user and can't change `/usr/local/bin`. To let it update itself, install it into a directory that user owns:

```sh
sudo install -d -o sideporch -g sideporch /opt/sideporch
curl -fsSL https://raw.githubusercontent.com/niklas-heer/sideporch/main/install.sh | sudo SIDEPORCH_INSTALL_DIR=/opt/sideporch sh
sudo chown sideporch:sideporch /opt/sideporch/sideporch
```

and in `/etc/systemd/system/sideporch.service`, run it from there and allow the write:

```ini
ExecStart=/opt/sideporch/sideporch --data /var/lib/sideporch --public-url https://chat.example.com
ReadWritePaths=/opt/sideporch
```

{% <note kind="warning"> %}
A server that may replace its own program is simpler to keep current, but whoever takes over the Sideporch process could replace it too. Updates themselves are only installed when their signature holds. If you'd rather keep the program read-only, keep the setup above and run `sudo sideporch update` when reminded.
{% </note> %}

Homebrew, Nix and container installs update the way they were installed; Sideporch shows how under Admin → Updates.

## What happens on start

On start, Sideporch applies every database change between the version you ran and the new one, in order, each in its own transaction. An interrupted upgrade picks up where it stopped.

Before it changes the database, Sideporch keeps a copy of it in `upgrade-backups/` in the data directory. It keeps the newest three copies.

## Go back to the version before

Stop Sideporch, put the program that ran before back, restore the database copy, and start it:

```sh
sudo systemctl stop sideporch
sudo mv /usr/local/bin/sideporch.previous /usr/local/bin/sideporch   # after sideporch update
sudo sideporch restore /var/lib/sideporch/upgrade-backups/sideporch-schema17-20261001-080000.db \
  --data /var/lib/sideporch --force
sudo chown -R sideporch:sideporch /var/lib/sideporch
sudo systemctl start sideporch
```

An older version refuses to start on a database a newer one has already upgraded, and names the version that did, rather than guessing at tables it doesn't know.

{% <note kind="warning"> %}
The copy holds the database as it was before the upgrade. Messages written since then are only in the current database, so take a [backup](@/docs/community/backups.md) of it first if you need them.
{% </note> %}
