# Service integration tests

These scripts test a running root `compose.yml` stack through its public protocols.

## Test accounts

Configure two password-enabled accounts, typically `alice` and `bob`. Each needs its own SSH key for `tests/git.sh`:

```toml
[[accounts]]
name = "alice"
ssh_keys = ["ssh-ed25519 ..."]
paths = ["alice/"]
password_hash = "$argon2id$..."

[[accounts]]
name = "bob"
ssh_keys = ["ssh-ed25519 ..."]
paths = ["bob/"]
password_hash = "$argon2id$..."
```

A full-suite run temporarily gives `alice` and `bob` the password `demo` and generated SSH keys, recreates the stack, then restores `config.toml` byte-for-byte and recreates the production stack.

## Run

```sh
tests/all.sh
# or: just test-services
```

Run one service test with `tests/auth.sh`, `tests/git.sh`, `tests/aur.sh`, `tests/irc.sh`, `tests/soju.sh`, `tests/xmpp.sh`, or `tests/mail.sh`.

Optional overrides: `AURCADE_HOST`, `AURCADE_DOMAIN`, `AURCADE_TLS`, `AURCADE_USER_A`, `AURCADE_USER_B`, `AURCADE_IRC_NETWORK`, and `AURCADE_MAIL_ALIAS`. The mail test otherwise discovers an alias that targets user A from `services/mail.toml`.

The tests create transient IRC/XMPP messages, mail, a Sieve script, a Git repository, and an AUR package repository. Mail and Sieve state are removed. Git deletion intentionally leaves small restore points in `.aurcade-trash`, because recoverable deletion is part of the tests.
