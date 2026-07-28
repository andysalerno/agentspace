FROM registry.opensuse.org/opensuse/distrobox:latest

RUN zypper --non-interactive install --no-recommends ca-certificates curl \
    && zypper clean --all \
    && curl --proto '=https' --tlsv1.2 -sSfL https://install.determinate.systems/nix -o /tmp/install-nix.sh \
    && sh /tmp/install-nix.sh install linux \
        --extra-conf "sandbox = false" \
        --no-start-daemon \
        --no-confirm \
    && curl -fsSL https://get.jetify.com/devbox -o /tmp/install-devbox.sh \
    && bash /tmp/install-devbox.sh --force \
    && chmod 0755 /usr/local/bin/devbox \
    && rm /tmp/install-devbox.sh /tmp/install-nix.sh

ENV PATH="/nix/var/nix/profiles/default/bin:${PATH}"
