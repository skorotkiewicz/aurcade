FROM alpine:3.21
RUN apk add --no-cache dovecot=2.3.21.1-r0 dovecot-lmtpd=2.3.21.1-r0 dovecot-pigeonhole-plugin=2.3.21.1-r0 \
    && addgroup -g 5000 vmail \
    && adduser -D -H -u 5000 -G vmail vmail
COPY docker/dovecot/dovecot.conf /etc/dovecot/dovecot.conf
COPY docker/dovecot/entrypoint.sh /usr/local/bin/aurcade-dovecot-entrypoint
RUN chmod +x /usr/local/bin/aurcade-dovecot-entrypoint
ENTRYPOINT ["/usr/local/bin/aurcade-dovecot-entrypoint"]
