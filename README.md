# AURcade

> Your little corner of the old internet. Insert SSH key to continue.

<p align="center">
  <img src="assets/screenshot.png" alt="AURcade cgit interface" width="80%">
</p>

AURcade puts Git, IRC, XMPP, and email in one Docker Compose stack. No quarters required.

## High scores

| | What you get |
| --- | --- |
| **Git** | cgit, SSH pushes, first-push repository creation, shared access, signed commits, an SSH lobby, and recoverable deletion |
| **AUR** | A private source-package server for `yay` and `paru`, backed by normal Git repositories |
| **IRC** | Ergo, a persistent Soju bouncer, and the Gamja web client |
| **XMPP** | Prosody, federation, native clients, WebSockets, and the Converse web client |
| **Mail** | Maddy SMTP/DKIM, Dovecot IMAP/Sieve, aliases, and SnappyMail webmail |
| **One account** | SSH keys for Git and one optional Argon2id password for chat and mail |
| **TLS** | Your certificate when configured, or a persistent self-signed certificate by default |

Repository metadata, push feedback, safe Markdown headers, and a signed-commit activity calendar come standard.

## Press start

You need Git, Docker, and Docker Compose.

```sh
git clone https://github.com/skorotkiewicz/aurcade.git
cd aurcade
cp config_example.toml config.toml
```

Generate your account block:

```sh
docker compose run --rm --no-deps aurcade account-template alice
```

Replace the example `[[accounts]]` block in `config.toml` with the printed block. Then set your `domain` and `clone_prefix`.

Start the cabinet:

```sh
docker compose up --build -d
```

Open <https://localhost:8080>. Your browser will grumble about the default self-signed certificate. It is doing its job.

## Pick a door

- **Git:** <https://localhost:8080>
- **SSH lobby:** `ssh -p 2222 git@localhost`
- **IRC:** <https://localhost:8080/chat/>
- **XMPP:** <https://localhost:8080/xmpp/>
- **Webmail:** <https://localhost:8080/mail/>

One account password works across IRC, XMPP, Soju, email, and webmail. Git over SSH remains key-only.

TLS is on by default. Plaintext mode exists for trusted local networks, because 1994 was lovely but had a smaller threat model.

Need every port, permission rule, DNS record, and configuration option? Read **[MORE.md](MORE.md)**.
