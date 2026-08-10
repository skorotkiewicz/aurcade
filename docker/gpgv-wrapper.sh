#!/bin/sh
set -eu

[ "$#" -eq 5 ]
[ "$1" = "--keyid-format=long" ]
[ "$2" = "--status-fd=1" ]
[ "$3" = "--verify" ]
exec /usr/bin/gpgv "$2" "$4" "$5"
