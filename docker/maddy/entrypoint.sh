#!/bin/sh
set -eu

services=/etc/aurcade/services
config=/tmp/maddy.conf
until [ -r "$services/maddy-domain" ] \
    && [ -r "$services/tls-enabled" ] \
    && [ -x "$services/maddy-auth" ]; do
    sleep 1
done

cp /etc/maddy/maddy.conf "$config"
if [ "$(cat "$services/tls-enabled")" = true ]; then
    until [ -r "$services/maddy-fullchain.pem" ] && [ -r "$services/maddy-privkey.pem" ]; do
        sleep 1
    done
else
    sed -i 's|^tls file .*|tls off|' "$config"
fi

MADDY_DOMAIN=$(cat "$services/maddy-domain")
export MADDY_DOMAIN
/bin/maddy -config "$config" verify-config
exec /bin/maddy -config "$config" run
