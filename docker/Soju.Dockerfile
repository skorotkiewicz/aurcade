FROM alpine:3.21
RUN apk add --no-cache soju=0.9.0-r2 soju-utils=0.9.0-r2 su-exec
COPY docker/soju-entrypoint.sh /usr/local/bin/aurcade-soju-entrypoint
RUN chmod +x /usr/local/bin/aurcade-soju-entrypoint
ENTRYPOINT ["/usr/local/bin/aurcade-soju-entrypoint"]
