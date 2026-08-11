#!/bin/sh
set -eu

services=/etc/aurcade/services
until [ -r "$services/maddy-domain" ] && [ -r "$services/tls-enabled" ]; do
    sleep 1
done
domain=$(cat "$services/maddy-domain")
tls=$(cat "$services/tls-enabled")
data=/var/lib/snappymail/_data_/_default_
config=$data/configs/application.ini

mail_ssl=
if [ "$tls" = true ]; then
    until [ -r "$services/maddy-fullchain.pem" ]; do
        sleep 1
    done
    cookie_secure=On
    imap_type=1
    smtp_type=2
    SECURE_COOKIES=true
    mail_ssl=$(cat <<EOF
,
        "ssl": {
            "verify_peer": true,
            "verify_peer_name": false,
            "allow_self_signed": true,
            "cafile": "$services/maddy-fullchain.pem",
            "SNI_enabled": true,
            "disable_compression": true,
            "security_level": 1
        }
EOF
)
else
    cookie_secure=Off
    imap_type=0
    smtp_type=0
    SECURE_COOKIES=false
    rm -f /usr/local/etc/php/conf.d/cookies.ini
fi
export SECURE_COOKIES

chown -R www-data:www-data /var/lib/snappymail
install -d -m 750 -o www-data -g www-data "$data/configs" "$data/domains"
if [ ! -f "$config" ]; then
    su - www-data -s /bin/sh -c 'php /snappymail/index.php' >/dev/null
fi

sed -i \
    -e 's|^title = .*|title = "AURcade Mail"|' \
    -e 's|^app_path = .*|app_path = "/mail/"|' \
    -e 's|^allow_admin_panel = .*|allow_admin_panel = Off|' \
    -e 's|^force_https = .*|force_https = Off|' \
    -e "s|^default_domain = .*|default_domain = \"$domain\"|" \
    -e 's|^attachment_size_limit = .*|attachment_size_limit = 25|' \
    -e 's|^cookie_default_path = .*|cookie_default_path = "/"|' \
    -e "s|^cookie_default_secure = .*|cookie_default_secure = $cookie_secure|" \
    "$config"

cat > "$data/domains/$domain.json" <<EOF
{
    "IMAP": {
        "host": "maddy",
        "port": 993,
        "type": $imap_type,
        "timeout": 30,
        "shortLogin": false,
        "lowerLogin": true,
        "sasl": ["PLAIN"]$mail_ssl
    },
    "SMTP": {
        "host": "maddy",
        "port": 587,
        "type": $smtp_type,
        "timeout": 30,
        "shortLogin": false,
        "lowerLogin": true,
        "sasl": ["PLAIN"]$mail_ssl,
        "useAuth": true,
        "setSender": true,
        "usePhpMail": false
    },
    "Sieve": {
        "host": "maddy",
        "port": 4190,
        "type": 0,
        "enabled": false
    },
    "whiteList": ""
}
EOF

chown -R www-data:www-data /var/lib/snappymail
exec /entrypoint.sh
