FROM python:3.13-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

# Install Copilot CLI and Claude Code (needed for harness kernels)
RUN apt-get update && \
    apt-get install -y --no-install-recommends curl && \
    curl -fsSL https://gh.io/copilot-install | bash && \
    curl -fsSL https://claude.ai/install.sh | bash && \
    curl -fsSL https://opencode.ai/install | bash && \
    apt-get purge -y curl && \
    apt-get autoremove -y && \
    rm -rf /var/lib/apt/lists/* && \
    ln -sf /root/.local/bin/claude /usr/local/bin/claude

ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy

WORKDIR /app

# Copy all kernel packages and root project config
COPY pyproject.toml pyproject.toml
COPY kernels/kernel kernels/kernel
COPY kernels/kernel_claude_code kernels/kernel_claude_code
COPY kernels/kernel_echo kernels/kernel_echo
COPY kernels/kernel_codex kernels/kernel_codex
COPY kernels/kernel_copilot kernels/kernel_copilot
COPY kernels/kernel_opencode kernels/kernel_opencode
COPY kernels/kernel_host kernels/kernel_host

# Install kernel_host and all its dependencies
RUN uv sync --no-dev --package kernel-host

# Create clean kernel workspace directory
RUN mkdir -p /workspace

ENTRYPOINT ["uv", "run", "--no-dev", "--package", "kernel-host", "-m", "kernel_host.runner"]
