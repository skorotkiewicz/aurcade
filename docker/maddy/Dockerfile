FROM ghcr.io/foxcpp/maddy:0.9.5@sha256:de42151adff6388edb5e4ee88f60334fa1ab85e309485193ecb1c2db20203315
COPY docker/maddy/maddy.conf /etc/maddy/maddy.conf
COPY docker/maddy/entrypoint.sh /usr/local/bin/aurcade-maddy-entrypoint
RUN chmod +x /usr/local/bin/aurcade-maddy-entrypoint
ENTRYPOINT ["/usr/local/bin/aurcade-maddy-entrypoint"]
