ARG CGIT_VERSION=1.3.1
ARG GIT_VERSION=2.54.0
ARG GAMJA_COMMIT=cdf94d62612e073cd632dca27779839f395153ec

FROM alpine:3.21 AS cgit-build
ARG CGIT_VERSION
ARG GIT_VERSION
WORKDIR /src
RUN apk add --no-cache build-base openssl-dev wget xz zlib-dev \
    && wget -q "https://git.zx2c4.com/cgit/snapshot/cgit-${CGIT_VERSION}.tar.xz" \
    && wget -q "https://www.kernel.org/pub/software/scm/git/git-${GIT_VERSION}.tar.gz" \
    && echo "aa5104f6597e67373af960ab9bbdf7f60b0bf9b19f7ca97ba71fa106709ae9e85b614f2a93302b09c50abdffd47e265bff18d3671fe704c9985efd38284a6b15  cgit-${CGIT_VERSION}.tar.xz" | sha512sum -c \
    && echo "944868c370c2e886eea347af9ee4d205baa630d2156e4c326fa3501bb343f5860256911f3ffc4b42f973f39efb7e641b86703e26088175cc41513f012e91769f  git-${GIT_VERSION}.tar.gz" | sha512sum -c \
    && tar -xf "cgit-${CGIT_VERSION}.tar.xz" \
    && tar -xf "git-${GIT_VERSION}.tar.gz" \
    && rm -rf "cgit-${CGIT_VERSION}/git" \
    && mv "git-${GIT_VERSION}" "cgit-${CGIT_VERSION}/git"
COPY docker/cgit-commit-signatures.patch /tmp/cgit-commit-signatures.patch
RUN cd "cgit-${CGIT_VERSION}" \
    && patch -p1 < /tmp/cgit-commit-signatures.patch \
    && make -j"$(nproc)" NO_LUA=1 NO_ICONV=YesPlease NO_GETTEXT=YesPlease \
        NO_TCLTK=YesPlease NO_SVN_TESTS=YesPlease NO_REGEX=NeedsStartEnd

FROM node:24.19.0-alpine AS gamja-build
ARG GAMJA_COMMIT
RUN apk add --no-cache git
WORKDIR /src
RUN git init \
    && git remote add origin https://codeberg.org/emersion/gamja.git \
    && git fetch --depth 1 origin "$GAMJA_COMMIT" \
    && git checkout --detach FETCH_HEAD \
    && test "$(git rev-parse HEAD)" = "$GAMJA_COMMIT"
RUN npm ci && npm run build

FROM node:24.19.0-alpine AS converse-build
RUN apk add --no-cache ca-certificates
WORKDIR /src
RUN wget -q "https://github.com/conversejs/converse.js/releases/download/v9.1.1/converse.js-9.1.1.tgz" \
    && tar -xzf converse.js-9.1.1.tgz \
    && mv package/dist dist

FROM rust:1.85-alpine AS rust-build
WORKDIR /src
RUN apk add --no-cache musl-dev
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --locked --release

FROM alpine:3.21
ARG CGIT_VERSION
ENV CGIT_CONFIG=/var/lib/aurcade/cgitrc
RUN apk add --no-cache git gpg gpg-agent gpgv highlight lighttpd mailcap openssh-server openssl py3-markdown py3-pygments \
    && adduser -D -h /home/git git \
    && install -d -o git -g git /home/git/.ssh /var/lib/aurcade /etc/aurcade
COPY --from=cgit-build /src/cgit-${CGIT_VERSION}/cgit /usr/share/webapps/cgit/cgit.cgi
COPY --from=cgit-build /src/cgit-${CGIT_VERSION}/cgit.css /src/cgit-${CGIT_VERSION}/cgit.js /src/cgit-${CGIT_VERSION}/cgit.png /src/cgit-${CGIT_VERSION}/favicon.ico /src/cgit-${CGIT_VERSION}/robots.txt /usr/share/webapps/cgit/
COPY --from=cgit-build /src/cgit-${CGIT_VERSION}/filters/syntax-highlighting.sh /usr/lib/cgit/filters/syntax-highlighting.sh
COPY --from=cgit-build /src/cgit-${CGIT_VERSION}/filters/about-formatting.sh /usr/lib/cgit/filters/about-formatting.sh
COPY --from=cgit-build /src/cgit-${CGIT_VERSION}/filters/html-converters/ /usr/lib/cgit/filters/html-converters/
COPY --from=rust-build /src/target/release/aurcade /usr/local/bin/aurcade
COPY --from=gamja-build /src/dist/ /usr/share/webapps/gamja/
COPY --from=converse-build /src/dist/ /usr/share/webapps/converse/
COPY docker/converse-index.html /usr/share/webapps/converse/index.html
COPY docker/lighttpd.conf /etc/lighttpd/lighttpd.conf
COPY docker/cgit-theme.css /usr/share/webapps/cgit/cgit-theme.css
COPY docker/aurcade-logo.svg /usr/share/webapps/cgit/aurcade-logo.svg
COPY docker/aurcade-favicon.svg /usr/share/webapps/cgit/aurcade-favicon.svg
COPY docker/about-filter.py /usr/local/bin/aurcade-about-filter
COPY docker/gpgv-wrapper.sh /usr/local/bin/aurcade-gpgv
COPY docker/entrypoint.sh /usr/local/bin/entrypoint
RUN passwd -d git \
    && sed -i 's/ -X / -O xhtml /' /usr/lib/cgit/filters/syntax-highlighting.sh \
    && highlight -O xhtml --style-outfile=stdout --print-style >> /usr/share/webapps/cgit/cgit.css \
    && chmod +x /usr/lib/cgit/filters/syntax-highlighting.sh /usr/local/bin/aurcade-about-filter /usr/local/bin/aurcade-gpgv /usr/local/bin/entrypoint \
    && printf '\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\nAllowUsers git\n' >> /etc/ssh/sshd_config
EXPOSE 22 80
VOLUME ["/var/lib/aurcade"]
ENTRYPOINT ["/usr/local/bin/entrypoint"]
