FROM prosodyim/prosody:13.0@sha256:e44a7dfedb776c8945b5c6401a3437a009b549923f7bbcb5a480c2de5f86e5d7
COPY docker/mod_auth_aurcade.lua /usr/lib/prosody/modules/mod_auth_aurcade.lua
COPY docker/prosody-entrypoint.sh /usr/local/bin/prosody-entrypoint
RUN chmod +x /usr/local/bin/prosody-entrypoint
ENTRYPOINT ["/usr/local/bin/prosody-entrypoint"]
