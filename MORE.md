# AURcade

AURcade is a small self-hosted Git, chat, and mail server. One TOML file controls accounts and access.

<p align="center">
  <img src="assets/screenshot.png" alt="AURcade cgit interface" width="80%">
</p>

## Features

### Git

- cgit web interface and HTTP clones
- SSH clones and authenticated pushes
- Repository creation on the first push to an owned namespace
- Exact repository grants and shared repository grants
- Read-only SSH lobby with a 53-week signed-commit calendar
- SSH and GPG signature verification
- Push results with commit and activity details
- Recoverable repository deletion
- Per-repository `.aurcade` descriptions
- Optional safe Markdown header on every cgit page

### Chat

- Ergo IRC server
- Soju IRC bouncer with persistent history
- Gamja IRC web client at `/chat/`
- Prosody XMPP server with federation
- Converse XMPP web client at `/xmpp/`
- Native IRC, XMPP, and WebSocket access

### Mail

- Maddy SMTP, submission, IMAP, DKIM, and local aliases
- SnappyMail webmail at `/mail/`
- Persistent mailboxes, queues, and DKIM keys
- Stored mail remains after account removal

### Accounts and security

- SSH uses public keys
- IRC, XMPP, Soju, email, and webmail use one optional Argon2id password
- TLS is enabled by default
- A configured certificate takes priority over the generated self-signed certificate
- Plaintext mode is available for isolated trusted networks

## Quick start

You need Git, Docker, and Docker Compose.

1. Clone the repository.

   ```sh
   git clone https://github.com/skorotkiewicz/aurcade.git
   cd aurcade
   ```

2. Copy the example configuration.

   ```sh
   cp configs/config_example.toml configs/config.toml
   ```

3. Edit `domain`, `clone_prefix`, and the site text in `configs/config.toml`.

4. Generate an account block.

   ```sh
   docker compose run --rm --no-deps aurcade account-template alice
   ```

5. Replace the example `[[accounts]]` block with the generated block.

6. Start AURcade.

   ```sh
   docker compose up --build -d
   ```

7. Open <https://localhost:8080>.

The default self-signed certificate causes a browser warning. Add a trusted certificate before public use.

Run this command after account, domain, TLS, or service configuration changes:

```sh
docker compose up -d --force-recreate
```

## Configuration

`configs/config.toml` is the account and site configuration:

```toml
title = "AURcade"
domain = "example.com"
tls = true
# tls_certificate = "tls/fullchain.pem"
# tls_private_key = "tls/privkey.pem"
description = "My Git repositories"
markdown_description = """
# Welcome

- [IRC](/chat/)
- [XMPP](/xmpp/)
- [Webmail](/mail/)
"""
clone_prefix = "https://example.com:8080 ssh://git@example.com:2222"
style = "cgit-theme.css"
logo = "aurcade-logo.svg"
favicon = "aurcade-favicon.svg"

[[accounts]]
name = "alice"
password_hash = "$argon2id$..."
ssh_keys = ["ssh-ed25519 AAAA..."]
gpg_keys = []
gpg_key_files = []
paths = ["alice/"]
```

AURcade escapes raw HTML in `markdown_description`.

Generate only a password hash with this command:

```sh
docker compose run --rm --no-deps aurcade hash-password
```

The account password is optional. Accounts without `password_hash` can use Git over SSH but cannot use password services.

## Repository access

An exact path grants access to one repository. A trailing slash grants a namespace and permits repository creation.

```toml
paths = ["example", "team/tools"]
paths = ["alice/"]
```

Give the same path to two accounts to make the repository shared. Shared repositories appear under `shared` in cgit.

Open the SSH lobby without a Git command:

```sh
ssh -p 2222 git@localhost
```

Clone an existing repository:

```sh
git clone ssh://git@localhost:2222/example.git
```

Create a repository with the first push:

```sh
git remote add origin ssh://git@localhost:2222/alice/newrepo.git
git push -u origin main
```

Soft-delete an owned, non-shared repository:

```sh
ssh -p 2222 git@localhost delete alice/old-repo --confirm alice/old-repo
```

AURcade moves the repository to `.aurcade-trash/` in the repository directory.

Add a plain-text `.aurcade` file to set a repository description:

```text
A small repository with a useful description.
```

AURcade refreshes metadata after each push.

## Signed commits

AURcade verifies SSH signatures against each account's `ssh_keys`.

Add complete armored GPG public keys directly:

```toml
gpg_keys = ['''
-----BEGIN PGP PUBLIC KEY BLOCK-----
...
-----END PGP PUBLIC KEY BLOCK-----
''']
```

You can also mount public-key files and reference them from `configs/config.toml`:

```toml
gpg_key_files = ["keys/alice.asc"]
```

Put these files in `keys/`. Each configured path must start with `keys/`.

Invalid or missing GPG keys cause a startup warning. They do not stop AURcade.

The SSH lobby calendar counts trusted signed commits on each repository's default branch.

## Chat configuration

The service files in `configs/` contain only service-specific settings:

```toml
# configs/irc.toml
network = "AURcade"
autojoin = ["#aurcade"]
```

```toml
# configs/xmpp.toml
admins = ["alice"]
```

```toml
# configs/soju.toml
admins = ["alice"]
```

Gamja connects through Soju. Browser and native bouncer clients share the same IRC session and history.

## Endpoints

| Service | TLS enabled | TLS disabled |
| --- | --- | --- |
| Web, cgit, and web clients | `https://HOST:8080` | `http://HOST:8080` |
| Git SSH and lobby | `HOST:2222` | `HOST:2222` |
| Ergo IRC | TLS on `6697` | Plaintext on `6697` |
| Soju IRC | TLS on `6698` | Plaintext on `6698` |
| XMPP clients | STARTTLS required on `5222` | Plaintext allowed on `5222` |
| XMPP federation | STARTTLS required on `5269` | Plaintext allowed on `5269` |
| Converse WebSocket | Same-origin `/xmpp-websocket` | `ws://DOMAIN:5280/xmpp-websocket` |
| Direct XMPP WebSocket | `wss://DOMAIN:5281/xmpp-websocket` | `ws://DOMAIN:5280/xmpp-websocket` |
| Incoming SMTP | Port `25`, STARTTLS available | Plaintext on `25` |
| Mail submission | Port `587`, STARTTLS required | Plaintext on `587` |
| IMAP | TLS on `993` | Plaintext on `993` |

Use `ACCOUNT@DOMAIN` to sign in to XMPP, email, and webmail. Use the account name for IRC and Soju.

## TLS

TLS defaults to `true`. AURcade generates a persistent self-signed certificate when certificate paths are absent.

To use your certificate, put both files in `aurcade_data/tls` and configure both paths:

```toml
tls = true
tls_certificate = "tls/fullchain.pem"
tls_private_key = "tls/privkey.pem"
```

AURcade uses the selected certificate for the web server, Ergo, Soju, Prosody, Maddy, and SnappyMail.

Set `tls = false` only on an isolated trusted network. AURcade rejects certificate paths in plaintext mode.

## Email configuration

Maddy creates a mailbox for each password-enabled account. `configs/mail.toml` defines the postmaster and local aliases:

```toml
postmaster = "alice"

[aliases]
support = "alice"
admin = "alice"
```

Alias targets must be password-enabled account names. The postmaster account receives mail sent to `postmaster@DOMAIN`.

AURcade does not create a catch-all alias. SnappyMail has no published port or administrator panel.

Before public email use, configure these DNS records:

- An `A` or `AAAA` record for `DOMAIN`
- An `MX` record that points to `DOMAIN`
- Matching reverse DNS
- SPF and DMARC records
- The generated DKIM record

After Maddy starts, read the DKIM value from:

```text
aurcade_data/srv/maddy_data/dkim_keys/DOMAIN_default.dns
```

Publish the value as the TXT record `default._domainkey.DOMAIN`.

## Persistent data

AURcade uses these host paths:

| Path | Contents |
| --- | --- |
| `configs/` | Site, account, chat, and mail configuration |
| `keys/` | Referenced GPG public keys |
| `aurcade_data/repositories/` | Git repositories and `.aurcade-trash/` |
| `aurcade_data/tls/` | Configured or generated TLS files |
| `aurcade_data/ssh_host_keys/` | SSH host identity |
| `aurcade_data/srv/ergo_data/` | Ergo state |
| `aurcade_data/srv/soju_data/` | Soju accounts and history |
| `aurcade_data/srv/prosody_data/` | Prosody state |
| `aurcade_data/srv/maddy_data/` | Mail, queues, and DKIM keys |
| `aurcade_data/srv/snappymail_data/` | SnappyMail state |

Copy `configs`, `keys`, and `aurcade_data` to backup storage. AURcade keeps stored mail after account removal.
