#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
require_passwords
wait_for_stack ergo
exec python3 tests/protocol.py irc
