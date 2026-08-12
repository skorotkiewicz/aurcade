#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
require_passwords
wait_for_stack maddy
wait_for_stack dovecot
if [[ -z ${AURCADE_MAIL_ALIAS:-} ]]; then
    AURCADE_MAIL_ALIAS=$(awk -F= -v target="$AURCADE_USER_A" '
        /^\[aliases\]/ { aliases=1; next }
        /^\[/ { aliases=0 }
        aliases {
            value=$2
            gsub(/[ \t\"]/, "", value)
            if (value == target) { key=$1; gsub(/[ \t]/, "", key); print key; exit }
        }
    ' services/mail.toml)
    export AURCADE_MAIL_ALIAS
fi
exec python3 tests/protocol.py mail
