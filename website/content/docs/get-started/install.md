+++
title = "Install"
description = "Install Sideporch on a server with the install script, or with Docker, Homebrew or Nix."
weight = 3
+++

Every release has prebuilt programs for Linux and macOS, on x86-64 and ARM64. The Linux ones are fully static: they need no libc or any other library, so they run on any distribution, from Debian to Alpine.

## On a server: the install script

For a server you'll keep running, use the install script. It installs one file and nothing else.

```sh
curl -fsSL https://raw.githubusercontent.com/niklas-heer/sideporch/main/install.sh | sh
```

The script:

- picks the build for your system and processor,
- downloads it from the [GitHub release](https://github.com/niklas-heer/sideporch/releases), and checks it against the release's `SHA256SUMS`,
- installs `sideporch` into `/usr/local/bin` when it may write there, otherwise into `~/.local/bin`.

Then [run it on the server](@/docs/get-started/run-on-a-server.md), usually as a systemd service.

To read the script before running it:

```sh
curl -fsSL https://raw.githubusercontent.com/niklas-heer/sideporch/main/install.sh -o install.sh
less install.sh
sh install.sh
```

Environment variables change what it does:

| Variable | Default | Meaning |
| --- | --- | --- |
| `SIDEPORCH_VERSION` | the latest release | A version to install, such as `0.4.0`. |
| `SIDEPORCH_INSTALL_DIR` | `/usr/local/bin` or `~/.local/bin` | Where to put the program. |
| `SIDEPORCH_DOWNLOAD_URL` | GitHub | Where the release files are, for a mirror. |

For example, as root: `curl -fsSL …/install.sh | SIDEPORCH_INSTALL_DIR=/usr/local/bin sh`.

## Docker

The image holds nothing but the program. It listens on port 8080, keeps its data in `/data`, and runs as user 10001.

```sh
docker run -d --name sideporch --restart unless-stopped \
  -p 127.0.0.1:8080:8080 -v sideporch:/data ghcr.io/niklas-heer/sideporch
```

Or with Docker Compose:

```yaml
services:
  sideporch:
    image: ghcr.io/niklas-heer/sideporch
    restart: unless-stopped
    ports:
      - "127.0.0.1:8080:8080"
    volumes:
      - sideporch:/data
    environment:
      SIDEPORCH_PUBLIC_URL: https://chat.example.com
volumes:
  sideporch:
```

Images are tagged `latest`, each version (`0.4.0`) and each minor version (`0.4`), and `main` for the newest development build. Both x86-64 and ARM64 are published. The image has no shell; run commands with `docker exec sideporch /sideporch …`.

## Homebrew

On macOS and Linux:

```sh
brew install niklas-heer/tap/sideporch
brew services start sideporch   # keep it running, with data in $(brew --prefix)/var/sideporch
```

## Nix and NixOS

```sh
nix run github:niklas-heer/sideporch
```

On NixOS, import the flake's module and turn it on. It runs Sideporch as a systemd service with its data in `/var/lib/sideporch`:

```nix
{
  inputs.sideporch.url = "github:niklas-heer/sideporch";

  outputs = { nixpkgs, sideporch, ... }: {
    nixosConfigurations.chat = nixpkgs.lib.nixosSystem {
      modules = [
        sideporch.nixosModules.default
        {
          services.sideporch = {
            enable = true;
            listen = "127.0.0.1:8080";
            publicUrl = "https://chat.example.com";
          };
        }
      ];
    };
  };
}
```

## From source

With a Rust toolchain (see `rust-toolchain.toml` for the version):

```sh
git clone https://github.com/niklas-heer/sideporch
cd sideporch
cargo build --release
./target/release/sideporch
```

No Node.js or other build tools are needed.
