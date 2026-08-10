FROM alpine:3.22 AS build
RUN apk add --no-cache build-base libidn2-dev lua5.4-dev openssl-dev
WORKDIR /src
COPY resources/prosody-im ./
RUN ./configure --prefix=/usr --sysconfdir=/etc/prosody --datadir=/var/lib/prosody \
        --lua-version=5.4 --idn-library=idn --with-random=getrandom \
    && make -j"$(nproc)" \
    && make install DESTDIR=/out

FROM alpine:3.22
RUN apk add --no-cache libidn2 lua5.4 lua5.4-cqueues lua5.4-expat lua5.4-filesystem lua5.4-sec lua5.4-socket openssl \
    && addgroup -S prosody \
    && adduser -S -D -H -G prosody prosody \
    && install -d -o prosody -g prosody /var/lib/prosody /var/run/prosody /usr/lib/prosody/custom_plugins
COPY --from=build /out/ /
COPY docker/mod_auth_aurcade.lua /usr/lib/prosody/custom_plugins/mod_auth_aurcade.lua
COPY docker/prosody-entrypoint.sh /usr/local/bin/prosody-entrypoint
RUN chmod +x /usr/local/bin/prosody-entrypoint
EXPOSE 5222 5269
VOLUME ["/var/lib/prosody"]
ENTRYPOINT ["/usr/local/bin/prosody-entrypoint"]
