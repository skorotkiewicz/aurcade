#!/bin/sh
set -eu

config=/etc/aurcade/services/ircd.yaml
until [ -r "$config" ]; do
    sleep 1
done
cd /var/lib/ergo
exec /ircd-bin/ergo run --conf "$config"
