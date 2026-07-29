FROM registry.opensuse.org/opensuse/tumbleweed:latest

ARG PODMAN_COMPOSE_VERSION=1.6.0
ARG VSCODE_CLI_RELEASE=commit:e4c7e7b1d6d060162f4aa7f8225271b67ce1df75

ENV CARGO_HOME="/home/dev/.cargo" \
    LANG="C.UTF-8" \
    PATH="/home/dev/.local/bin:/home/dev/.cargo/bin:${PATH}" \
    RUSTUP_HOME="/usr/local/lib/rustup" \
    UV_PYTHON_DOWNLOADS="never" \
    UV_TOOL_BIN_DIR="/usr/local/bin" \
    UV_TOOL_DIR="/opt/uv-tools" \
    VSCODE_CLI_DATA_DIR="/home/dev/.vscode-cli" \
    VSCODE_CLI_USE_FILE_KEYCHAIN="1"

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

RUN case "$(uname -m)" in \
        x86_64) vscode_arch="x64" ;; \
        aarch64|arm64) vscode_arch="arm64" ;; \
        *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;; \
    esac \
    && curl --fail --location --silent --show-error \
        "https://update.code.visualstudio.com/${VSCODE_CLI_RELEASE}/cli-linux-${vscode_arch}/stable" \
        --output /tmp/vscode-cli.tar.gz \
    && tar -xzf /tmp/vscode-cli.tar.gz -C /usr/local/bin code \
    && rm /tmp/vscode-cli.tar.gz

RUN printf '%s\n' 'export PATH="/home/dev/.local/bin:$PATH"' \
    > /etc/bash.bashrc.local \
    && git config --system --add \
        url.https://github.com/.insteadOf \
        git@github.com: \
    && git config --system --add \
        url.https://github.com/.insteadOf \
        ssh://git@github.com/ \
    && git config --system \
        credential.https://github.com.helper \
        "!/usr/bin/gh auth git-credential"

WORKDIR /workspace

STOPSIGNAL SIGINT

ENTRYPOINT ["code", "tunnel", \
    "--accept-server-license-terms", \
    "--name", "agentspace-dev", \
    "--cli-data-dir", "/home/dev/.vscode-cli", \
    "--server-data-dir", "/home/dev/.vscode-server", \
    "--extensions-dir", "/home/dev/.vscode-server/extensions"]
