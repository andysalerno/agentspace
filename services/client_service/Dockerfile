FROM python:3.13-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

WORKDIR /app

COPY pyproject.toml pyproject.toml
COPY kernels/kernel kernels/kernel
COPY kernels/kernel_claude_code kernels/kernel_claude_code
COPY kernels/kernel_echo kernels/kernel_echo
COPY kernels/kernel_codex kernels/kernel_codex
COPY kernels/kernel_copilot kernels/kernel_copilot
COPY kernels/kernel_host kernels/kernel_host
COPY services/client_service services/client_service

RUN uv sync --no-dev --package client-service

EXPOSE 8002

CMD ["/usr/local/bin/uv", "run", "--no-dev", "--package", "client-service", "-m", "client_service"]
