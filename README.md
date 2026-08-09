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

3. Start the service.

   ```sh
   docker compose up --build -d
   ```

4. Open <http://localhost:8080>.

The SSH service uses port `2222` on the host.

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
