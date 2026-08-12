#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
require_passwords
wait_for_stack soju
export AURCADE_IRC_NETWORK=${AURCADE_IRC_NETWORK:-$(sed -n 's/^network = "\(.*\)"/\1/p' services/irc.toml)}
exec python3 tests/protocol.py soju
