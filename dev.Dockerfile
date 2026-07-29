FROM registry.opensuse.org/opensuse/tumbleweed:latest

ENV CARGO_HOME="/home/dev/.cargo" \
    LANG="C.UTF-8" \
    PATH="/home/dev/.local/bin:/home/dev/.cargo/bin:${PATH}" \
    RUSTUP_HOME="/usr/local/lib/rustup"

RUN zypper --non-interactive install --no-recommends \
        ca-certificates \
        curl \
        gcc \
        gcc-c++ \
        git \
        just \
        make \
        nodejs26 \
        openssh-clients \
        pkgconf-pkg-config \
        pnpm \
        python313 \
        python313-devel \
        python313-uv \
        rustup \
        tmux \
        vim \
    && zypper clean --all \
    && install -d -m 1777 /run/tmux \
    && rustup toolchain install stable \
        --profile minimal \
        --component clippy,rustfmt \
    && rustup default stable

RUN printf '%s\n' 'export PATH="/home/dev/.local/bin:$PATH"' \
    > /etc/bash.bashrc.local

WORKDIR /workspace

CMD ["/bin/bash"]
