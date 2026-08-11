FROM djmaze/snappymail:v2.38.2@sha256:5e3d990438809a8a49f8ac5758db03e858e6e9fc0e369e1f9e474f7664079905
COPY docker/snappymail/entrypoint.sh /usr/local/bin/aurcade-snappymail-entrypoint
RUN chmod +x /usr/local/bin/aurcade-snappymail-entrypoint
ENTRYPOINT ["/usr/local/bin/aurcade-snappymail-entrypoint"]
