#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

wait_for_services() {
    local service attempt
    for attempt in {1..120}; do
        for service in aurcade ergo prosody soju dovecot maddy snappymail; do
            [[ $(docker inspect -f '{{.State.Running}}' "$service" 2>/dev/null) == true ]] || break
        done
        [[ $service == snappymail ]] \
            && [[ $(docker inspect -f '{{.State.Health.Status}}' aurcade dovecot maddy snappymail 2>/dev/null | grep -c '^healthy$') == 4 ]] \
            && return
        sleep 1
    done
    docker compose ps >&2
    return 1
}

backup=$(mktemp /tmp/aurcade-config.XXXXXX)
keys=$(mktemp -d /tmp/aurcade-test-keys.XXXXXX)
cp config.toml "$backup"

restore() {
    local status=$?
    trap - EXIT
    cp "$backup" config.toml
    echo 'Restoring production configuration...'
    if ! docker compose up -d --force-recreate || ! wait_for_services; then
        echo 'failed to restart the restored production stack' >&2
        status=1
    fi
    rm -rf "$backup" "$keys"
    exit "$status"
}
trap restore EXIT

ssh-keygen -q -t ed25519 -N '' -f "$keys/alice"
ssh-keygen -q -t ed25519 -N '' -f "$keys/bob"
python3 - "$keys/alice.pub" "$keys/bob.pub" <<'PY'
from pathlib import Path
import json
import re
import sys

path = Path("config.toml")
parts = re.split(r"(?=\[\[accounts\]\])", path.read_text())
password_hashes = {
    "alice": "$argon2id$v=19$m=19456,t=2,p=1$2e7j3yYratAFuqyAZ7gWjw$8n4MVRvRGiuy3DQKJAbFSJozShqdAMsd2gVQs7BgpvM",
    "bob": "$argon2id$v=19$m=19456,t=2,p=1$DOWLmz0SlKBQp675zhIJtw$RlvZhW3EcN8r8CatJ/Zc/sTGpTe4YIr9AtExcMJfVAM",
}
keys = {
    "alice": Path(sys.argv[1]).read_text().strip(),
    "bob": Path(sys.argv[2]).read_text().strip(),
}
updated = set()
for index, part in enumerate(parts):
    match = re.search(r'^name = "([^"]+)"$', part, re.MULTILINE)
    if not match or match.group(1) not in keys:
        continue
    account = match.group(1)
    key = json.dumps(keys[account])
    part, key_count = re.subn(
        r"^ssh_keys = \[(.*)\]$",
        lambda match: f"ssh_keys = [{match.group(1)}, {key}]",
        part,
        count=1,
        flags=re.MULTILINE,
    )
    part, password_count = re.subn(
        r'^password_hash = ".*"$',
        f'password_hash = "{password_hashes[account]}"',
        part,
        count=1,
        flags=re.MULTILINE,
    )
    if key_count != 1 or password_count != 1:
        raise SystemExit(f"account {account} needs one ssh_keys and password_hash line")
    parts[index] = part
    updated.add(account)
if updated != keys.keys():
    raise SystemExit("config.toml must contain alice and bob accounts")
path.write_text("".join(parts))
PY

export AURCADE_PASSWORD_A=demo_a AURCADE_PASSWORD_B=demo_b
export AURCADE_SSH_KEY_A="$keys/alice" AURCADE_SSH_KEY_B="$keys/bob"
echo 'Starting temporary Alice/Bob test fixture...'
docker compose up -d --force-recreate
wait_for_services

for test in auth irc soju xmpp mail git aur; do
    printf '\n==> tests/%s.sh\n' "$test"
    "tests/$test.sh"
done
