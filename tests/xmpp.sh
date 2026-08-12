#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
require_passwords
wait_for_stack prosody
exec python3 tests/protocol.py xmpp
