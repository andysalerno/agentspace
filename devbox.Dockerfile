FROM registry.opensuse.org/opensuse/tumbleweed:latest

RUN zypper --non-interactive install --no-recommends ca-certificates curl git just vim \
    && zypper clean --all \
    && curl --proto '=https' --tlsv1.2 -sSfL https://install.determinate.systems/nix -o /tmp/install-nix.sh \
    && sh /tmp/install-nix.sh install linux \
        --extra-conf "sandbox = false" \
        --init none \
        --no-confirm \
    && curl -fsSL https://get.jetify.com/devbox -o /tmp/install-devbox.sh \
    && bash /tmp/install-devbox.sh --force \
    && chmod 0755 /usr/local/bin/devbox \
    && rm /tmp/install-devbox.sh /tmp/install-nix.sh

RUN printf '%s\n' 'export PATH="/home/devbox/.local/bin:$PATH"' \
    > /etc/bash.bashrc.local

ENV LANG="C.UTF-8" \
    PATH="/home/devbox/.local/bin:/nix/var/nix/profiles/default/bin:${PATH}"

WORKDIR /workspace

CMD ["/bin/bash"]
