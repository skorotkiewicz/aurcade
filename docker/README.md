# AURcade

<p align="center">
  <img src="assets/aurcade-banner.svg" alt="AURcade retro arcade cabinet — insert SSH key to continue" width="100%">
</p>

A small Git repository host for static accounts.

- cgit provides the web interface and HTTP clones.
- SSH provides authenticated Git access.
- `config.toml` defines accounts, SSH keys, and repository paths.

## Start

1. Copy the example configuration.

   ```sh
   cp config_example.toml config.toml
   ```

2. Add each account and its public SSH keys to `config.toml`.

3. Pull and start the published image, `ghcr.io/skorotkiewicz/aurcade`.

   ```sh
   docker compose pull
   docker compose up -d
   ```

   To build locally instead:

   ```sh
   docker compose up --build -d
   ```

4. Open <http://localhost:8080>.

The SSH service uses port `2222` on the host.

## Docker run

Create a directory:

```sh
mkdir aurcade
cd aurcade
```

Create `config.toml` with:

```toml
title = "AURcade"
description = "My Git repositories"
clone_prefix = "http://localhost:8080 ssh://git@localhost:2222"
style = "cgit-theme.css"
logo = "aurcade-logo.svg"

[[accounts]]
name = "alice"
ssh_keys = ["ssh-ed25519 REPLACE_WITH_YOUR_PUBLIC_KEY"]
gpg_keys = []
gpg_key_files = []
paths = ["alice/"]
```

Run the published image:

```sh
docker run -d \
  --name aurcade \
  --restart unless-stopped \
  -p 8080:80 \
  -p 2222:22 \
  -v "$PWD/config.toml:/etc/aurcade/config.toml:ro" \
  -v "$PWD/keys:/etc/aurcade/keys:ro" \
  -v aurcade-repositories:/var/lib/aurcade \
  -v aurcade-ssh-host-keys:/etc/ssh/host_keys \
  ghcr.io/skorotkiewicz/aurcade
```

Docker creates the two named volumes automatically.

## Configure access

An exact path grants access to one repository:

```toml
paths = ["example", "team/tools"]
```

A path with a trailing slash grants access to a namespace. It also permits repository creation on the first push:

```toml
paths = ["alice/"]
```

Restart the service after each configuration change:

```sh
docker compose restart
```

## Repository metadata

Commit `.aurcade` to a repository's default branch to set its cgit description:

```console
A tiny repository with an unnecessarily dramatic README
```

Metadata is refreshed after each successful push. Repositories writable by one account appear in that account's cgit section; repositories writable by multiple accounts appear under `shared`.

## Use SSH

Clone an existing repository:

```sh
git clone ssh://git@localhost:2222/example.git
```

Create a repository in an allowed namespace:

```sh
git remote add origin ssh://git@localhost:2222/alice/newrepo.git
git push -u origin main
```

Repository data and SSH host keys remain in `repositories/` and `ssh_host_keys/`.
