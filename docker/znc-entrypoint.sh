#!/bin/sh
set -eu

config=/etc/aurcade/services/znc.conf
certificate=/etc/aurcade/services/znc.pem
until [ -r "$config" ] && [ -r "$certificate" ]; do
    sleep 1
done
install -d -m 700 /znc-data/configs
install -m 600 "$config" /znc-data/configs/znc.conf
install -m 600 "$certificate" /znc-data/znc.pem
exec /entrypoint.sh "$@"
