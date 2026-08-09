#!/bin/sh
set -eu

: "${AUR_REPOS_ROOT:=/var/lib/aur-repos}"
export AUR_REPOS_ROOT

aur-repos setup
chown -R git:git "$AUR_REPOS_ROOT" /home/git/.ssh
host_key=/etc/ssh/host_keys/ssh_host_ed25519_key
install -d -m 700 /etc/ssh/host_keys
[ -f "$host_key" ] || ssh-keygen -q -t ed25519 -N '' -f "$host_key"
/usr/sbin/sshd -h "$host_key"
exec lighttpd -D -f /etc/lighttpd/lighttpd.conf
