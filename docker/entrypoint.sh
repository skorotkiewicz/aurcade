#!/bin/sh
set -eu

: "${AURCADE_ROOT:=/var/lib/aurcade}"
export AURCADE_ROOT

if [ "$#" -gt 0 ]; then
    exec aurcade "$@"
fi

aurcade setup
if [ -f /etc/aurcade/irc.toml ] && [ -f /etc/aurcade/xmpp.toml ]; then
    aurcade generate-services
    cp /etc/aurcade/services/gamja-config.json /usr/share/webapps/gamja/config.json
fi
install -d -m 700 -o lighttpd -g lighttpd /var/cache/cgit
find /var/cache/cgit -mindepth 1 -delete
chown -R git:git "$AURCADE_ROOT" /home/git/.ssh
chown -R lighttpd:lighttpd /etc/aurcade/signing
host_key=/etc/ssh/host_keys/ssh_host_ed25519_key
install -d -m 700 /etc/ssh/host_keys
[ -f "$host_key" ] || ssh-keygen -q -t ed25519 -N '' -f "$host_key"
/usr/sbin/sshd -D -e -h "$host_key" &
sshd_pid=$!
lighttpd -D -f /etc/lighttpd/lighttpd.conf &
lighttpd_pid=$!
aurcade auth-server &
auth_pid=$!
trap 'kill "$sshd_pid" "$lighttpd_pid" "$auth_pid" 2>/dev/null || true' INT TERM EXIT
while kill -0 "$sshd_pid" 2>/dev/null && kill -0 "$lighttpd_pid" 2>/dev/null && kill -0 "$auth_pid" 2>/dev/null; do
    sleep 1
done
exit 1
