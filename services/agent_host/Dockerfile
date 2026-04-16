FROM python:3.13-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy

WORKDIR /app

COPY pyproject.toml pyproject.toml
COPY kernels/kernel kernels/kernel
COPY kernels/kernel_claude_code kernels/kernel_claude_code
COPY kernels/kernel_echo kernels/kernel_echo
COPY kernels/kernel_codex kernels/kernel_codex
COPY kernels/kernel_copilot kernels/kernel_copilot
COPY kernels/kernel_opencode kernels/kernel_opencode
COPY kernels/kernel_host kernels/kernel_host
COPY services/agent_host services/agent_host

RUN uv sync --no-dev --package agent-host

ENTRYPOINT ["uv", "run", "--no-dev", "--package", "agent-host", "-m", "agent_host"]
