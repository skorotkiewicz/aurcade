#!/bin/sh
set -eu

: "${AUR_REPOS_ROOT:=/var/lib/aur-repos}"
export AUR_REPOS_ROOT

aur-repos setup
chown -R git:git "$AUR_REPOS_ROOT" /home/git/.ssh
ssh-keygen -A
/usr/sbin/sshd
exec lighttpd -D -f /etc/lighttpd/lighttpd.conf
