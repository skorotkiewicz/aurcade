#!/bin/sh
set -eu

services=/etc/aurcade/services
config=/etc/maddy/maddy.conf
until [ -r "$services/maddy-domain" ] \
    && [ -r "$services/maddy-users" ] \
    && [ -r "$services/maddy-fullchain.pem" ] \
    && [ -r "$services/maddy-privkey.pem" ] \
    && [ -x "$services/maddy-auth" ]; do
    sleep 1
done

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
