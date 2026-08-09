#!/bin/sh
set -eu

: "${AURCADE_ROOT:=/var/lib/aurcade}"
export AURCADE_ROOT

aurcade setup
chown -R git:git "$AURCADE_ROOT" /home/git/.ssh
host_key=/etc/ssh/host_keys/ssh_host_ed25519_key
install -d -m 700 /etc/ssh/host_keys
[ -f "$host_key" ] || ssh-keygen -q -t ed25519 -N '' -f "$host_key"
/usr/sbin/sshd -h "$host_key"
exec lighttpd -D -f /etc/lighttpd/lighttpd.conf
