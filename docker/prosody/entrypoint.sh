#!/bin/sh
set -eu

services=/etc/aurcade/services
config=$services/prosody.cfg.lua
until [ -r "$config" ] && [ -r "$services/tls-enabled" ]; do
    sleep 1
done
install -d -m 700 -o prosody -g prosody /var/run/prosody/tls
if [ "$(cat "$services/tls-enabled")" = true ]; then
    install -m 644 -o prosody -g prosody "$services/prosody-fullchain.pem" /var/run/prosody/tls/fullchain.pem
    install -m 600 -o prosody -g prosody "$services/prosody-privkey.pem" /var/run/prosody/tls/privkey.pem
fi
chown -R prosody:prosody /var/lib/prosody /var/run/prosody
exec runuser -u prosody -- /usr/bin/prosody --config "$config"
