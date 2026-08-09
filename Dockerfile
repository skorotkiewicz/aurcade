FROM rust:1.85-alpine AS build
WORKDIR /src
RUN apk add --no-cache musl-dev
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release

FROM alpine:3.21
ENV CGIT_CONFIG=/var/lib/aur-repos/cgitrc
RUN apk add --no-cache cgit git highlight lighttpd openssh-server \
    && sed -i 's/ -X / -O xhtml /' /usr/lib/cgit/filters/syntax-highlighting.sh \
    && highlight -O xhtml --style-outfile=stdout --print-style >> /usr/share/webapps/cgit/cgit.css \
    && adduser -D -h /home/git git \
    && install -d -o git -g git /home/git/.ssh /var/lib/aur-repos /etc/aur-repos
COPY --from=build /src/target/release/aur-repos /usr/local/bin/aur-repos
COPY lighttpd.conf /etc/lighttpd/lighttpd.conf
COPY cgit-theme.css /usr/share/webapps/cgit/cgit-theme.css
COPY entrypoint.sh /usr/local/bin/entrypoint
RUN passwd -d git \
    && chmod +x /usr/local/bin/entrypoint \
    && printf '\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\nAllowUsers git\n' >> /etc/ssh/sshd_config
EXPOSE 22 80
VOLUME ["/var/lib/aur-repos"]
ENTRYPOINT ["/usr/local/bin/entrypoint"]
