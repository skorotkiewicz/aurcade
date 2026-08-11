# AURcade

<!--<p align="center">
  <img src="assets/aurcade-banner.svg" alt="AURcade retro arcade cabinet — insert SSH key to continue" width="100%">
</p>-->

A tiny Git host with cgit over HTTP and authenticated pushes over SSH.

<p align="center">
  <img src="assets/screenshot.png" alt="AURcade cgit interface" width="80%">
</p>

- cgit provides the web interface and HTTP clones; an optional `markdown_description` in `config.toml` renders as a header block on every page.
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
domain = "example.com"
# Defaults to true. Set false only for an isolated, trusted network.
tls = true
# With TLS enabled, omit both to generate persistent self-signed files under ./tls.
# tls_certificate = "tls/fullchain.pem"
# tls_private_key = "tls/privkey.pem"
description = "My Git repositories"
# Optional Markdown rendered as a block at the top of every cgit page.
# markdown_description = """\
# # AURcade

# A tiny **Git host** with cgit over HTTP and authenticated pushes over SSH.

# - [SSH Lobby](ssh://git@localhost:2222) · read-only, no shell
# - IRC **#aurcade** on port `6697` · [web client](/chat/)
# - XMPP `alice@chat.localhost` on port `5222` · [web client](/xmpp/)
# """
clone_prefix = "https://localhost:8080 ssh://git@localhost:2222"
style = "cgit-theme.css"
logo = "aurcade-logo.svg"
favicon = "aurcade-favicon.svg"

[[accounts]]
name = "alice"
password_hash = "$argon2id$REPLACE_WITH_GENERATED_HASH"
ssh_keys = ["ssh-ed25519 REPLACE_WITH_YOUR_PUBLIC_KEY"]
gpg_keys = []
gpg_key_files = []
paths = ["alice/"]
```

Generate a ready-to-paste account block or only an Argon2id password hash:

```sh
docker compose run --rm --no-deps aurcade account-template alice
docker compose run --rm --no-deps aurcade hash-password
```

These commands print TOML without modifying `config.toml`.

```sh
docker run -d \
  --name aurcade \
  --restart unless-stopped \
  -p 8080:443 \
  -p 2222:22 \
  -v "$PWD/config.toml:/etc/aurcade/config.toml:ro" \
  -v "$PWD/keys:/etc/aurcade/keys:ro" \
  -v "$PWD/tls:/etc/aurcade/tls" \
  -v ./repositories:/var/lib/aurcade \
  -v ./ssh-host-keys:/etc/ssh/host_keys \
  ghcr.io/skorotkiewicz/aurcade:latest
```

With Compose:

```sh
docker compose up --build -d
```

Open <https://localhost:8080>. The generated certificate is self-signed unless configured otherwise, so browsers may require confirmation. With `tls = false`, use <http://localhost:8080>; all published web, IRC, XMPP, submission, and IMAP endpoints become plaintext on their existing ports. Use this mode only on an isolated, trusted network. After account, domain, or service configuration changes, run `docker compose up -d --force-recreate`.

## Access

```toml
paths = ["example", "team/tools"] # An exact path grants access to one repository.
paths = ["alice/"]  # Namespace; creates repositories on first push.
```

Connect without a Git command to open the account's SSH lobby. Its 53-week activity calendar counts only commits on repository default branches with trusted SSH signatures belonging to that account:

```sh
ssh -p 2222 git@localhost
```

## Git

```sh
git clone ssh://git@localhost:2222/example.git

git remote add origin ssh://git@localhost:2222/alice/newrepo.git
git push -u origin main
```

Soft-delete an owned, non-shared repository by moving it into `.aurcade-trash/` inside the repository mount:

```sh
ssh -p 2222 git@localhost delete alice/old-repo --confirm alice/old-repo
```

## Chat

Compose runs Ergo IRC and Prosody XMPP with the same account names and `password_hash` values from `config.toml`. Configure service-specific options separately:

```toml
# irc.toml
network = "AURcade"
autojoin = ["#aurcade"]
```

```toml
# xmpp.toml
admins = ["alice"]
```

```toml
# soju.toml
admins = ["alice"]
```

Endpoints:

- Gamja: <https://localhost:8080/chat/>
- Converse.js XMPP: <https://localhost:8080/xmpp/>
- IRC TLS: `localhost:6697`
- Soju IRC TLS: `localhost:6698`
- XMPP clients: `alice@DOMAIN` on port `5222`
- XMPP over WebSocket: `wss://localhost:5281/xmpp-websocket`, or `ws://localhost:5280/xmpp-websocket` with `tls = false`
- XMPP federation: port `5269`

Gamja connects through Soju, so browser and native bouncer clients share one upstream session and persistent history in `soju_data`. Accounts and passwords remain authoritative in `config.toml`; administrators are listed in `soju.toml`.

TLS defaults to enabled. AURcade first uses both configured global TLS paths. When they are omitted, it generates a persistent self-signed certificate in `./aurcade_data/tls`. The selected certificate is used by the web server, Ergo, Soju, Prosody, Maddy, and SnappyMail:

```toml
tls = true
tls_certificate = "tls/fullchain.pem"
tls_private_key = "tls/privkey.pem"
```

Set `tls = false` for plaintext-only operation. Certificate paths are not accepted in that mode.

## Email

Maddy creates a mailbox for each account with a `password_hash`. Configure the postmaster and optional local aliases separately:

```toml
# mail.toml
postmaster = "alice"

[aliases]
support = "alice"
admin = "alice"
```

Alias targets must be password-enabled account names. Use the full address, such as `alice@DOMAIN`, and the same password as IRC/XMPP.

- Incoming SMTP: port `25` with STARTTLS when available
- Authenticated submission: port `587` with STARTTLS required before authentication
- IMAP: port `993` with TLS
- Webmail: <https://localhost:8080/mail/>

Mail, queues, and generated DKIM keys persist in `./aurcade_data/srv/maddy_data`; SnappyMail state persists in `./aurcade_data/srv/snappymail_data`. The configured postmaster receives mail addressed to `postmaster`. Restart the stack after changing accounts or `mail.toml`; removed accounts retain their stored mail.

Before using public email, configure an `A`/`AAAA` record, an `MX` record pointing at `DOMAIN`, matching reverse DNS, SPF, and DMARC. After Maddy starts, publish the value from `./aurcade_data/srv/maddy_data/dkim_keys/DOMAIN_default.dns` as the TXT record `default._domainkey.DOMAIN`.

## Signed commits

SSH-signed commits are verified with the account's `ssh_keys`. For GPG signatures, add complete armored public keys:

```toml
gpg_keys = ['''
-----BEGIN PGP PUBLIC KEY BLOCK-----
...
-----END PGP PUBLIC KEY BLOCK-----
''']
```

Export one with `gpg --armor --export FINGERPRINT`. Alternatively, reference public-key files available inside the container:

```toml
gpg_key_files = ["keys/alice.asc"]
```

Key-file paths must begin with `keys/`, relative to `config.toml`. Invalid or missing GPG keys are ignored with a startup warning. Restart AURcade after changing keys.

## Metadata

Add `.aurcade` to a repository:

```console
A tiny repository with an unnecessarily dramatic README
```

Metadata refreshes after each push. Repositories shared by multiple accounts appear under `shared`.
