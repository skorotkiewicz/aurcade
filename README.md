# AURcade

<p align="center">
  <img src="assets/aurcade-banner.svg" alt="AURcade retro arcade cabinet — insert SSH key to continue" width="100%">
</p>

A tiny Git host with cgit over HTTP and authenticated pushes over SSH.

- cgit provides the web interface and HTTP clones.
- SSH provides authenticated Git access.
- `config.toml` defines accounts, SSH keys, and repository paths.

## Run

Create a directory:

```sh
mkdir aurcade
cd aurcade
```

Create `config.toml`:

```toml
title = "AURcade"
description = "My Git repositories"
clone_prefix = "http://localhost:8080 ssh://git@localhost:2222"
style = "cgit-theme.css"
logo = "aurcade-logo.svg"

[[accounts]]
name = "alice"
ssh_keys = ["ssh-ed25519 REPLACE_WITH_YOUR_PUBLIC_KEY"]
paths = ["alice/"]
```

```sh
docker run -d \
  --name aurcade \
  --restart unless-stopped \
  -p 8080:80 \
  -p 2222:22 \
  -v "$PWD/config.toml:/etc/aurcade/config.toml:ro" \
  -v ./repositories:/var/lib/aurcade \
  -v ./ssh-host-keys:/etc/ssh/host_keys \
  ghcr.io/skorotkiewicz/aurcade:latest
```

Build locally instead:

```sh
docker compose -f docker/docker-compose.yml up --build -d
```

Open <http://localhost:8080>. After configuration changes, run `docker restart aurcade`.

## Access

```toml
paths = ["example", "team/tools"] # An exact path grants access to one repository.
paths = ["alice/"]  # Namespace; creates repositories on first push.
```

## Git

```sh
git clone ssh://git@localhost:2222/example.git

git remote add origin ssh://git@localhost:2222/alice/newrepo.git
git push -u origin main
```

## Metadata

Add `.aurcade.toml` to a repository:

```toml
description = "A tiny repository with an unnecessarily dramatic README"
```

Metadata refreshes after each push. Repositories shared by multiple accounts appear under `shared`.
