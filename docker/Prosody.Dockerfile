FROM prosodyim/prosody:13.0
COPY docker/mod_auth_aurcade.lua /usr/lib/prosody/modules/mod_auth_aurcade.lua
COPY docker/prosody-entrypoint.sh /usr/local/bin/prosody-entrypoint
RUN chmod +x /usr/local/bin/prosody-entrypoint
ENTRYPOINT ["/usr/local/bin/prosody-entrypoint"]
