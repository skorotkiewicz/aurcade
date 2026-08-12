#!/bin/sh
set -eu

services=/etc/aurcade/services
until [ -r "$services/dovecot-users" ] \
    && [ -r "$services/maddy-domain" ] \
    && [ -r "$services/tls-enabled" ]; do
    sleep 1
done

install -d -m 750 -o vmail -g dovecot /var/lib/dovecot/users
config=/tmp/dovecot.conf
cp /etc/dovecot/dovecot.conf "$config"
domain=$(cat "$services/maddy-domain")
sed -i "s/postmaster@localhost/postmaster@$domain/" "$config"

if [ "$(cat "$services/tls-enabled")" = true ]; then
    until [ -r "$services/maddy-fullchain.pem" ] && [ -r "$services/maddy-privkey.pem" ]; do
        sleep 1
    done
else
    sed -i \
        -e 's/^disable_plaintext_auth = yes/disable_plaintext_auth = no/' \
        -e 's/^ssl = required/ssl = no/' \
        -e '/^ssl_cert =/d' \
        -e '/^ssl_key =/d' \
        -e '/inet_listener imap {/,/}/s/port = 0/port = 993/' \
        -e '/inet_listener imaps {/,/}/s/port = 993/port = 0/' \
        -e '/ssl = yes/d' \
        "$config"
fi

exec dovecot -F -c "$config"
