#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
require_passwords
wait_for_stack aurcade
wait_for_stack dovecot

check_user() {
    local user=$1 password=$2 address="$1@$AURCADE_DOMAIN" ergo
    printf '%s\n%s\n' "$address" "$password" \
        | docker compose exec -T aurcade aurcade auth-maddy
    ergo=$(jq -nc --arg user "$user" --arg password "$password" \
        '{accountName:$user, passphrase:$password}')
    printf '%s\n' "$ergo" | docker compose exec -T aurcade aurcade auth-ergo \
        | jq -e --arg user "$user" '.success == true and .accountName == $user' >/dev/null
    docker compose exec -T -e TEST_USER="$user" -e TEST_PASSWORD="$password" aurcade sh -c '
        wget -qS -O /dev/null --post-data="$TEST_PASSWORD" \
            --header="X-Aurcade-Account: $TEST_USER" http://127.0.0.1:9000/verify 2>&1 \
            | grep -F "204 No Content" >/dev/null
        wget -qS -O /dev/null --post-data="" \
            --header="X-Aurcade-Account: $TEST_USER" http://127.0.0.1:9000/exists 2>&1 \
            | grep -F "204 No Content" >/dev/null
        basic=$(printf "%s:%s" "$TEST_USER" "$TEST_PASSWORD" | base64)
        wget -qS -O /dev/null --post-data="" \
            --header="Authorization: Basic $basic" http://127.0.0.1:9000/soju 2>&1 \
            | grep -F "200 OK" >/dev/null
    '
    docker compose exec -T dovecot doveadm -c /tmp/dovecot.conf auth test "$address" "$password" \
        | grep -F 'auth succeeded' >/dev/null
}

check_user "$AURCADE_USER_A" "$AURCADE_PASSWORD_A"
check_user "$AURCADE_USER_B" "$AURCADE_PASSWORD_B"

if printf '%s\n%s\n' "$AURCADE_USER_A@$AURCADE_DOMAIN" wrong \
    | docker compose exec -T aurcade aurcade auth-maddy; then
    echo 'bad mail password accepted' >&2
    exit 1
fi
if docker compose exec -T dovecot doveadm -c /tmp/dovecot.conf auth test \
    "$AURCADE_USER_A@$AURCADE_DOMAIN" wrong >/dev/null 2>&1; then
    echo 'bad Dovecot password accepted' >&2
    exit 1
fi
docker compose exec -T -e TEST_USER="$AURCADE_USER_A" aurcade sh -c '
    status=$(wget -S -O /dev/null --post-data=wrong \
        --header="X-Aurcade-Account: $TEST_USER" http://127.0.0.1:9000/verify 2>&1 || true)
    printf "%s" "$status" | grep -F "401 Unauthorized" >/dev/null
    basic=$(printf "%s:wrong" "$TEST_USER" | base64)
    status=$(wget -S -O /dev/null --post-data="" \
        --header="Authorization: Basic $basic" http://127.0.0.1:9000/soju 2>&1 || true)
    printf "%s" "$status" | grep -F "403 Forbidden" >/dev/null
'

pass 'central HTTP, Ergo, Maddy, Soju-contract, and Dovecot authentication'
