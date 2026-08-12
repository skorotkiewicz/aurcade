FROM ghcr.io/foxcpp/maddy:0.9.5@sha256:de42151adff6388edb5e4ee88f60334fa1ab85e309485193ecb1c2db20203315 AS maddy

FROM alpine:3.21
RUN apk add --no-cache dovecot=2.3.21.1-r0 dovecot-lmtpd=2.3.21.1-r0 dovecot-pigeonhole-plugin=2.3.21.1-r0 sqlite \
    && addgroup -g 5000 vmail \
    && adduser -D -H -u 5000 -G vmail vmail
COPY --from=maddy /bin/maddy /bin/maddy
COPY docker/dovecot/dovecot.conf /etc/dovecot/dovecot.conf
COPY docker/dovecot/migration-maddy.conf /etc/maddy/migration.conf
COPY docker/dovecot/entrypoint.sh /usr/local/bin/aurcade-dovecot-entrypoint
COPY docker/dovecot/migrate.sh /usr/local/bin/aurcade-mail-migrate
RUN chmod +x /usr/local/bin/aurcade-dovecot-entrypoint /usr/local/bin/aurcade-mail-migrate
ENTRYPOINT ["/usr/local/bin/aurcade-dovecot-entrypoint"]
