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

Start the stack, then export credentials without committing them:

```sh
export AURCADE_PASSWORD_A='alice password'
export AURCADE_PASSWORD_B='bob password'
export AURCADE_SSH_KEY_A="$HOME/.ssh/alice"
export AURCADE_SSH_KEY_B="$HOME/.ssh/bob"
```

If both accounts deliberately use the same password, `AURCADE_PASSWORD` sets both.

## Run

```sh
tests/all.sh
# or: just test-services
```

The full suite prompts for any missing passwords and SSH private-key paths when run in a terminal. Non-interactive runs must export them first.

Run one service test with `tests/auth.sh`, `tests/git.sh`, `tests/irc.sh`, `tests/soju.sh`, `tests/xmpp.sh`, or `tests/mail.sh`.

Optional overrides: `AURCADE_HOST`, `AURCADE_DOMAIN`, `AURCADE_TLS`, `AURCADE_USER_A`, `AURCADE_USER_B`, `AURCADE_IRC_NETWORK`, and `AURCADE_MAIL_ALIAS`. The mail test otherwise discovers an alias that targets user A from `services/mail.toml`.

The tests create transient IRC/XMPP messages, mail, a Sieve script, and a Git repository. Mail and Sieve state are removed. Git deletion intentionally leaves its small restore point in `.aurcade-trash`, because recoverable deletion is part of the test.
