FROM registry.opensuse.org/opensuse/tumbleweed:latest

ENV CARGO_HOME="/opt/cargo" \
    LANG="C.UTF-8" \
    PATH="/home/dev/.local/bin:${PATH}" \
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
        pkgconf-pkg-config \
        pnpm \
        python313 \
        python313-devel \
        python313-uv \
        rustup \
        tmux \
        vim \
    && zypper clean --all \
    && rustup toolchain install stable \
        --profile minimal \
        --component clippy,rustfmt \
    && rustup default stable

RUN printf '%s\n' 'export PATH="/home/dev/.local/bin:$PATH"' \
    > /etc/bash.bashrc.local

WORKDIR /workspace

COPY . .

RUN uv sync --all-packages --dev --locked \
    && pnpm --dir clients/webui install --frozen-lockfile \
    && just check

CMD ["/bin/bash"]
