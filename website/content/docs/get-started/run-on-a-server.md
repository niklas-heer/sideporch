+++
title = "Run it on a server"
description = "Run Sideporch as a service, create the admin account safely, and put it behind HTTPS."
weight = 4
+++

This page takes Sideporch from an installed program to a chat people can reach: a service that starts with the server, the first account, and HTTPS.

## Start it as a service

With the [install script](@/docs/get-started/install.md#on-a-server-the-install-script), run Sideporch with systemd. Create a user for it:

```sh
sudo useradd --system --home-dir /var/lib/sideporch --shell /usr/sbin/nologin sideporch
```

Save this as `/etc/systemd/system/sideporch.service`, with your own address in `--public-url`:

```ini
[Unit]
Description=Sideporch team chat
After=network-online.target
Wants=network-online.target

[Service]
User=sideporch
Group=sideporch
ExecStart=/usr/local/bin/sideporch --data /var/lib/sideporch --public-url https://chat.example.com --require-setup-link
StateDirectory=sideporch
StateDirectoryMode=0700
Restart=on-failure
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

Then start it, and have it start with the server:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now sideporch
journalctl -u sideporch -f    # its log
```

Sideporch listens on `127.0.0.1:8080`, so only the server itself reaches it until you [add HTTPS](#put-it-behind-https).

## Create the admin account

While no account exists, the first visitor creates the admin account. On a server others can reach, start Sideporch with `--require-setup-link` (as above): then creating the first account needs a one-time link, which stays out of service and container logs. Get it on the server:

```sh
sudo -u sideporch sideporch setup-link --data /var/lib/sideporch
docker exec sideporch /sideporch setup-link    # with Docker
```

Open the link, create your account, and invite everyone else from **People**. To let people join without an invite, open sign-up under **Admin → Community**; see [Sign-up and trust](@/docs/community/people/sign-up-and-trust.md).

## Options

Every option can also be set with an environment variable.

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--listen` | `SIDEPORCH_LISTEN` | `127.0.0.1:8080` | Address and port to listen on. |
| `--data` | `SIDEPORCH_DATA` | `sideporch-data` | Directory for everything Sideporch keeps. Back it up to back up everything. |
| `--public-url` | `SIDEPORCH_PUBLIC_URL` | taken from each request | The URL people use, such as `https://chat.example.com`, for invite and webhook links. An `https://` URL also makes cookies secure. |
| `--require-setup-link` | `SIDEPORCH_REQUIRE_SETUP_LINK` | off | Require the one-time link from `sideporch setup-link` to create the first account. |
| `--client-ip-header` | `SIDEPORCH_CLIENT_IP_HEADER` | none: the connection's address | Behind a reverse proxy, the header it puts visitors' addresses in, such as `X-Forwarded-For` or `Fly-Client-IP`. Sign-in limits and [bans](@/docs/community/people/moderation.md#bans) use it. Only set it when every request comes through that proxy, or anyone could claim any address. New in 0.6.0. |
| `--update-check` | `SIDEPORCH_UPDATE_CHECK` | `true` | Ask GitHub for new releases every six hours; `false` keeps Sideporch from contacting GitHub. See [Update](@/docs/get-started/update.md). New in 0.5.0. |
| | `SIDEPORCH_SECRET_KEY` | `secret.key` in the data directory | A passphrase to encrypt [automation secrets](@/docs/integrations/automations/data-and-services.md#secrets) with, instead of the key file. |
| | `RUST_LOG` | `info` | How much to log: `warn`, `info`, `debug`. |

Three commands besides running the server:

- `sideporch setup-link` prints the link for creating the first account.
- `sideporch restore ARCHIVE` unpacks a [backup](@/docs/community/server/backups.md). Stop the server first.
- `sideporch update` installs the newest release in place of the program; see [Update](@/docs/get-started/update.md#sideporch-update). New in 0.5.0.

## Put it behind HTTPS

Browsers only send push notifications, install Sideporch as an app and use passkeys over HTTPS, so put a reverse proxy in front of it. Live updates use a WebSocket at `/ws`, which the proxy has to pass on.

With [Caddy](https://caddyserver.com), which gets certificates by itself, this is the whole configuration:

```
chat.example.com {
	reverse_proxy 127.0.0.1:8080
}
```

With nginx:

```nginx
server {
    listen 443 ssl;
    server_name chat.example.com;
    # ssl_certificate and ssl_certificate_key, for example from certbot

    client_max_body_size 0;   # Sideporch limits uploads itself; Slack exports can be large

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $http_connection;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 1h;
    }
}
```

Set `--public-url` to the address people use. Without it, Sideporch builds links from the `Host` (or `X-Forwarded-Host`) and `X-Forwarded-Proto` headers the proxy passes on.

Set `--client-ip-header X-Forwarded-For` too, so Sideporch sees visitors' addresses instead of the proxy's; otherwise [sign-in limits](@/docs/community/people/sign-in-security.md#limits-on-guessing) and [bans](@/docs/community/people/moderation.md#bans) would treat everyone as one visitor. Caddy sets that header by itself; with nginx, add `proxy_set_header X-Forwarded-For $remote_addr;`.

## Check that it runs

`/healthz` answers `ok` while Sideporch runs, for monitors and container health checks:

```sh
curl -fsS http://127.0.0.1:8080/healthz
```

Next: [keep it up to date](@/docs/get-started/update.md), and set up [backups](@/docs/community/server/backups.md).
