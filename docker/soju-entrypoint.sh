#!/bin/sh
set -eu

config=/etc/aurcade/services/soju.conf
users=/etc/aurcade/services/soju-users
until [ -r "$config" ] && [ -r "$users" ]; do
    sleep 1
done
install -d -m 700 -o soju -g soju /run/soju /soju-data /soju-data/tls
install -m 644 -o soju -g soju /etc/aurcade/services/soju-fullchain.pem /soju-data/tls/fullchain.pem
install -m 600 -o soju -g soju /etc/aurcade/services/soju-privkey.pem /soju-data/tls/privkey.pem
chown -R soju:soju /soju-data
su-exec soju soju -config "$config" &
pid=$!
trap 'kill "$pid" 2>/dev/null || true' INT TERM EXIT
until [ -S /run/soju/admin ]; do
    kill -0 "$pid"
    sleep 1
done
while IFS="	" read -r account admin network; do
    ctl="su-exec soju sojuctl -config $config"
    $ctl user create -username "$account" -disable-password -admin "$admin" >/dev/null 2>&1 \
        || $ctl user update "$account" -disable-password -admin "$admin" >/dev/null
    $ctl user run "$account" network create -addr irc://ergo:6667 -name "$network" -nick "$account" >/dev/null 2>&1 \
        || $ctl user run "$account" network update "$network" -addr irc://ergo:6667 -nick "$account" >/dev/null
 done < "$users"
wait "$pid"
