FROM registry.opensuse.org/opensuse/distrobox:latest

RUN zypper --non-interactive install --no-recommends ca-certificates curl \
    && zypper clean --all \
    && curl -fsSL https://get.jetify.com/devbox -o /tmp/install-devbox.sh \
    && bash /tmp/install-devbox.sh --force \
    && chmod 0755 /usr/local/bin/devbox \
    && rm /tmp/install-devbox.sh
