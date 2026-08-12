#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

export AURCADE_HOST=${AURCADE_HOST:-127.0.0.1}
export AURCADE_USER_A=${AURCADE_USER_A:-alice}
export AURCADE_USER_B=${AURCADE_USER_B:-bob}
export AURCADE_PASSWORD_A=${AURCADE_PASSWORD_A:-${AURCADE_PASSWORD:-}}
export AURCADE_PASSWORD_B=${AURCADE_PASSWORD_B:-${AURCADE_PASSWORD:-}}
[[ $AURCADE_USER_A =~ ^[A-Za-z0-9_-]+$ && $AURCADE_USER_B =~ ^[A-Za-z0-9_-]+$ ]] \
    || { echo 'test account names must contain only letters, digits, _ or -' >&2; exit 2; }
export AURCADE_DOMAIN=${AURCADE_DOMAIN:-$(cat aurcade_data/srv/service_config/maddy-domain)}
export AURCADE_TLS=${AURCADE_TLS:-$(cat aurcade_data/srv/service_config/tls-enabled)}
export PYTHONDONTWRITEBYTECODE=1

require() {
    local name
    for name; do
        [[ -n ${!name:-} ]] || { echo "required environment variable: $name" >&2; exit 2; }
    done
}

require_passwords() {
    require AURCADE_PASSWORD_A AURCADE_PASSWORD_B
}

wait_for_stack() {
    docker compose ps --status running --services | grep -qx aurcade
    docker compose ps --status running --services | grep -qx "$1"
}

pass() {
    printf 'PASS %s\n' "$1"
}
