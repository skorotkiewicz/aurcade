#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

prompt() {
    local name=$1 label=$2 secret=${3:-} value
    [[ -n ${!name:-} ]] && return
    [[ -t 0 ]] || {
        echo "required environment variable: $name (or run interactively)" >&2
        exit 2
    }
    if [[ $secret == secret ]]; then
        read -rsp "$label: " value
        echo
    else
        read -rp "$label: " value
    fi
    printf -v "$name" '%s' "$value"
}

prompt AURCADE_PASSWORD_A "Password for $AURCADE_USER_A" secret
prompt AURCADE_PASSWORD_B "Password for $AURCADE_USER_B" secret
prompt AURCADE_SSH_KEY_A "SSH private key for $AURCADE_USER_A"
prompt AURCADE_SSH_KEY_B "SSH private key for $AURCADE_USER_B"
export AURCADE_PASSWORD_A AURCADE_PASSWORD_B AURCADE_SSH_KEY_A AURCADE_SSH_KEY_B

for test in auth irc soju xmpp mail git; do
    printf '\n==> tests/%s.sh\n' "$test"
    "tests/$test.sh"
done
