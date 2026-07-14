# syntax=docker/dockerfile:1

FROM docker.io/library/python:3.13-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/
COPY --from=ghcr.io/casey/just:latest /just /usr/local/bin/

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
    curl -fsSL https://gh.io/copilot-install | bash && \
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

WORKDIR /app

# Copy all kernel packages and root project config
COPY pyproject.toml uv.lock ./
COPY kernels/kernel kernels/kernel
COPY kernels/kernel_claude_code kernels/kernel_claude_code
COPY kernels/kernel_echo kernels/kernel_echo
COPY kernels/kernel_codex kernels/kernel_codex
COPY kernels/kernel_copilot kernels/kernel_copilot
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
