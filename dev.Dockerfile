FROM registry.opensuse.org/opensuse/tumbleweed:latest

ARG PODMAN_COMPOSE_VERSION=1.6.0

ENV CARGO_HOME="/home/dev/.cargo" \
    LANG="C.UTF-8" \
    PATH="/home/dev/.local/bin:/home/dev/.cargo/bin:${PATH}" \
    RUSTUP_HOME="/usr/local/lib/rustup" \
    UV_PYTHON_DOWNLOADS="never" \
    UV_TOOL_BIN_DIR="/usr/local/bin" \
    UV_TOOL_DIR="/opt/uv-tools"

RUN zypper --non-interactive install --no-recommends \
        ca-certificates \
        curl \
        gcc \
        gcc-c++ \
        gh \
        git \
        just \
        make \
        nodejs26 \
        openssh-clients \
        pkgconf-pkg-config \
        pnpm \
        podman \
        python313 \
        python313-devel \
        python313-uv \
        rustup \
        tmux \
        vim \
    && zypper clean --all \
    && uv tool install \
        --python /usr/bin/python3.13 \
        "podman-compose==${PODMAN_COMPOSE_VERSION}" \
    && install -d -m 0755 /run/podman \
    && install -d -m 1777 /run/tmux \
    && rustup toolchain install stable \
        --profile minimal \
        --component clippy,rustfmt \
    && rustup default stable

RUN printf '%s\n' 'export PATH="/home/dev/.local/bin:$PATH"' \
    > /etc/bash.bashrc.local

WORKDIR /workspace

CMD ["/bin/bash"]
