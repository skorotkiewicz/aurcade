FROM ghcr.io/ergochat/ergo:v2.19.1@sha256:ef885e44f7fa19101bbbc41baef11dc280dc8107465dccaf6f0860f41b48a682
COPY docker/ergo-entrypoint.sh /usr/local/bin/ergo-entrypoint
RUN chmod +x /usr/local/bin/ergo-entrypoint
ENTRYPOINT ["/usr/local/bin/ergo-entrypoint"]
