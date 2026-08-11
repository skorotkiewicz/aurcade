#!/bin/sh
set -eu

services=/etc/aurcade/services
until [ -r "$services/maddy-domain" ] && [ -r "$services/maddy-fullchain.pem" ]; do
    sleep 1
done
domain=$(cat "$services/maddy-domain")
data=/var/lib/snappymail/_data_/_default_
config=$data/configs/application.ini

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
    -e 's|^cookie_default_secure = .*|cookie_default_secure = On|' \
    "$config"

cat > "$data/domains/$domain.json" <<EOF
{
    "IMAP": {
        "host": "maddy",
        "port": 993,
        "type": 1,
        "timeout": 30,
        "shortLogin": false,
        "lowerLogin": true,
        "sasl": ["PLAIN"],
        "ssl": {
            "verify_peer": true,
            "verify_peer_name": false,
            "allow_self_signed": true,
            "cafile": "$services/maddy-fullchain.pem",
            "SNI_enabled": true,
            "disable_compression": true,
            "security_level": 1
        }
    },
    "SMTP": {
        "host": "maddy",
        "port": 587,
        "type": 2,
        "timeout": 30,
        "shortLogin": false,
        "lowerLogin": true,
        "sasl": ["PLAIN"],
        "ssl": {
            "verify_peer": true,
            "verify_peer_name": false,
            "allow_self_signed": true,
            "cafile": "$services/maddy-fullchain.pem",
            "SNI_enabled": true,
            "disable_compression": true,
            "security_level": 1
        },
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
