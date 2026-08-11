#!/bin/sh
set -eu

config=/etc/aurcade/services/prosody.cfg.lua
until [ -r "$config" ]; do
    sleep 1
done
install -d -m 700 -o prosody -g prosody /var/run/prosody/tls
install -m 644 -o prosody -g prosody /etc/aurcade/services/prosody-fullchain.pem /var/run/prosody/tls/fullchain.pem
install -m 600 -o prosody -g prosody /etc/aurcade/services/prosody-privkey.pem /var/run/prosody/tls/privkey.pem
chown -R prosody:prosody /var/lib/prosody /var/run/prosody
exec runuser -u prosody -- /usr/bin/prosody --config "$config"
