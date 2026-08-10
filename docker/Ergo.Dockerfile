FROM golang:1.26-alpine3.22 AS build
RUN apk add --no-cache make
WORKDIR /src
COPY resources/ergo ./
RUN make install

FROM alpine:3.22
RUN apk add --no-cache ca-certificates
COPY --from=build /go/bin/ergo /ircd-bin/ergo
COPY --from=build /src/languages /ircd-bin/languages
COPY docker/ergo-entrypoint.sh /usr/local/bin/ergo-entrypoint
RUN chmod +x /usr/local/bin/ergo-entrypoint \
    && install -d /var/lib/ergo
EXPOSE 6697 8067
VOLUME ["/var/lib/ergo"]
ENTRYPOINT ["/usr/local/bin/ergo-entrypoint"]
