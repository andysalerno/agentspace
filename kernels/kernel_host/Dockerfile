# syntax=docker/dockerfile:1

FROM scratch AS memory-rs
COPY services/memory_rs /

FROM docker.io/library/rust:1.96-slim-bookworm AS memory-builder

ARG RUST_BUILD_PROFILE=release

WORKDIR /usr/src/agentspace

COPY Cargo.toml Cargo.lock ./
COPY services/agent_host_rs services/agent_host_rs
COPY services/client_service_rs services/client_service_rs
COPY --from=memory-rs . services/memory_rs
RUN --mount=type=cache,id=agentspace-kernel-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=agentspace-kernel-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    case "$RUST_BUILD_PROFILE" in \
        release) cargo build --locked --release --package memory_rs --bin memory \
            && cp target/release/memory /usr/local/bin/memory ;; \
        debug) cargo build --locked --package memory_rs --bin memory \
            && cp target/debug/memory /usr/local/bin/memory ;; \
        *) echo "unsupported RUST_BUILD_PROFILE=$RUST_BUILD_PROFILE" >&2; exit 1 ;; \
    esac

FROM docker.io/library/python:3.13-slim

ARG COPILOT_CLI_VERSION=v1.0.73

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/
COPY --from=ghcr.io/casey/just:latest /just /usr/local/bin/
COPY --from=memory-builder /usr/local/bin/memory /usr/local/bin/memory

# Install agent CLIs plus generally useful shell, build, and inspection tools.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        bash \
        build-essential \
        ca-certificates \
        coreutils \
        curl \
        file \
        findutils \
        gawk \
        git \
        gnupg \
        grep \
        gzip \
        jq \
        less \
        procps \
        ripgrep \
        sed \
        tar \
        unzip \
        wget \
        xz-utils \
        zip && \
    curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && \
    apt-get install -y --no-install-recommends nodejs && \
    curl -fsSL https://gh.io/copilot-install | VERSION="${COPILOT_CLI_VERSION}" bash && \
    curl -fsSL https://claude.ai/install.sh | bash && \
    curl -fsSL https://opencode.ai/install | bash -s -- --version 1.17.20 && \
    npm install -g firecrawl-cli && \
    curl -fsSL https://code-server.dev/install.sh | \
        sh -s -- --method=standalone --prefix=/usr/local && \
    apt-get autoremove -y && \
    rm -rf /var/lib/apt/lists/* && \
    ln -sf /root/.local/bin/claude /usr/local/bin/claude

# Ensure opencode is in PATH
RUN ln -sf /root/.opencode/bin/opencode /usr/local/bin/opencode

ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy
ENV AGENTSPACE_MEMORY_DIR=/var/lib/agentspace/memory

WORKDIR /app

# Copy all kernel packages and root project config
COPY pyproject.toml uv.lock ./
COPY kernels/kernel kernels/kernel
COPY kernels/kernel_claude_code kernels/kernel_claude_code
COPY kernels/kernel_echo kernels/kernel_echo
COPY kernels/kernel_codex kernels/kernel_codex
COPY kernels/kernel_opencode kernels/kernel_opencode
COPY kernels/kernel_acp kernels/kernel_acp
COPY kernels/kernel_host kernels/kernel_host

# Install kernel_host and all its dependencies
RUN --mount=type=cache,id=agentspace-uv-cache,target=/root/.cache/uv,sharing=locked \
    uv sync --frozen --no-dev --package kernel-host

# Create clean kernel workspace directory
RUN mkdir -p /workspace
EXPOSE 8000 8080 8081

ENTRYPOINT ["uv", "run", "--no-dev", "--package", "kernel-host", "-m", "kernel_host.runner"]

ARG AGENTSPACE_VERSION=dev
ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}
