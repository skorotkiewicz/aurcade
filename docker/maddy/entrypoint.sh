#!/bin/sh
set -eu

services=/etc/aurcade/services
config=/tmp/maddy.conf
until [ -r "$services/maddy-domain" ] \
    && [ -r "$services/maddy-users" ] \
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
    sed -i \
        -e 's|^tls file .*|tls off|' \
        -e 's|^imap tls://|imap tcp://|' \
        "$config"
fi

MADDY_DOMAIN=$(cat "$services/maddy-domain")
export MADDY_DOMAIN
/bin/maddy -config "$config" verify-config

existing=/tmp/maddy-users
/bin/maddy -config "$config" imap-acct list > "$existing"
while IFS= read -r user; do
    [ -n "$user" ] || continue
    grep -Fqx "$user" "$existing" || /bin/maddy -config "$config" imap-acct create "$user"
done < "$services/maddy-users"
rm -f "$existing"

# ponytail: removed accounts keep mail; add an explicit archival workflow before deletion.
exec /bin/maddy -config "$config" run
