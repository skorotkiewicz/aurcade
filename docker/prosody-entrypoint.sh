#!/bin/sh
set -eu

config=/etc/aurcade/services/prosody.cfg.lua
until [ -r "$config" ]; do
    sleep 1
done
chown -R prosody:prosody /var/lib/prosody /var/run/prosody
exec /usr/bin/prosody --config "$config"
