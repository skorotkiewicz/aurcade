#!/bin/sh
set -eu

old=/old-maddy
new=/var/lib/dovecot
marker=$new/.maddy-imported
[ ! -e "$marker" ] || exit 0
if nc -z -w 1 maddy 993 2>/dev/null; then
    echo "Legacy Maddy IMAP is running; stop maddy and snappymail before migration" >&2
    exit 1
fi
install -d -m 750 -o vmail -g vmail "$new/users"
[ -f "$old/imapsql.db" ] || { touch "$marker"; exit 0; }

maddy_state=/tmp/maddy-state
install -d -m 700 "$maddy_state"
cp "$old/imapsql.db" "$maddy_state/"
[ ! -f "$old/imapsql.db-wal" ] || cp "$old/imapsql.db-wal" "$maddy_state/"
[ ! -d "$old/messages" ] || cp -R "$old/messages" "$maddy_state/"

maddy=/bin/maddy
maddy_config=/etc/maddy/migration.conf

maddy_cmd() {
    "$maddy" -config "$maddy_config" "$@" 2>/dev/null
}

message_info() {
    maddy_cmd imap-msgs list --uid --full "$1" "$2" "$3"
}

users=$(maddy_cmd imap-acct list)
source_users=$(sqlite3 "$maddy_state/imapsql.db" 'select count(*) from users')
source_messages=$(sqlite3 "$maddy_state/imapsql.db" 'select count(*) from msgs')
[ "$source_users" = 0 ] || [ -n "$users" ] || { echo "Maddy snapshot contains users but none could be read" >&2; exit 1; }

for user in $users; do
    home="$new/users/$user"
    root="$home/Maildir"
    install -d -m 700 -o vmail -g vmail "$root/cur" "$root/new" "$root/tmp"
    : > "$root/subscriptions"
    maddy_cmd imap-mboxes list "$user" | while IFS="$(printf '\t')" read -r mailbox _attributes; do
        [ -n "$mailbox" ] || continue
        if [ "$mailbox" = INBOX ]; then
            maildir=$root
        else
            # ponytail: Maddy uses standard flat folders; add Maildir++ escaping if custom names need it.
            maildir="$root/.$(printf '%s' "$mailbox" | tr '/' '.')"
            printf '%s\n' "$mailbox" >> "$root/subscriptions"
        fi
        install -d -m 700 -o vmail -g vmail "$maildir/cur" "$maildir/new" "$maildir/tmp"
        maddy_cmd imap-msgs list --uid "$user" "$mailbox" \
            | sed -n 's/^UID \([0-9][0-9]*\):.*/\1/p' \
            | while read -r uid; do
                info=$(message_info "$user" "$mailbox" "$uid")
                received=$(printf '%s\n' "$info" | sed -n 's/^Internal date: \([0-9][0-9]*\) .*$/\1/p')
                flags=$(printf '%s\n' "$info" | sed -n 's/^Flags: \[\(.*\)\]$/\1/p' | sed 's/\\Recent//; s/^ *//; s/ *$//')
                standard=
                case " $flags " in *' \Draft '*) standard="${standard}D" ;; esac
                case " $flags " in *' \Flagged '*) standard="${standard}F" ;; esac
                case " $flags " in *' \Answered '*) standard="${standard}R" ;; esac
                case " $flags " in *' \Seen '*) standard="${standard}S" ;; esac
                case " $flags " in *' \Deleted '*) standard="${standard}T" ;; esac
                message="$maildir/cur/${received}.M${uid}.aurcade:2,$standard"
                maddy_cmd imap-msgs dump --uid "$user" "$mailbox" "$uid" > "$message"
                touch -d "@$received" "$message"
                chown vmail:vmail "$message"
            done
    done
    chown -R vmail:vmail "$home"
done

chown -R vmail:vmail "$new/users"
exported_messages=$(find "$new/users" -type f -name '*.aurcade:2,*' | wc -l | tr -d ' ')
[ "$exported_messages" = "$source_messages" ] || {
    echo "Maddy migration exported $exported_messages of $source_messages messages" >&2
    exit 1
}
touch "$marker"
