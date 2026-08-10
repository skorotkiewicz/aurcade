FROM znc:1.9.1@sha256:787973a70b044d23cc3df1b82eddb54a394a26aba7d7a05205e500fabd3d9c7d
COPY docker/znc-entrypoint.sh /usr/local/bin/aurcade-znc-entrypoint
RUN chmod +x /usr/local/bin/aurcade-znc-entrypoint
ENTRYPOINT ["/usr/local/bin/aurcade-znc-entrypoint"]
