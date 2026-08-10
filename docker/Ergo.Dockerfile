FROM ghcr.io/ergochat/ergo:v2.19.1
COPY docker/ergo-entrypoint.sh /usr/local/bin/ergo-entrypoint
RUN chmod +x /usr/local/bin/ergo-entrypoint
ENTRYPOINT ["/usr/local/bin/ergo-entrypoint"]
