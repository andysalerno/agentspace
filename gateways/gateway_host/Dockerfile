FROM python:3.13-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy

WORKDIR /app

# Copy all gateway packages and root project config
COPY pyproject.toml pyproject.toml
COPY gateways/gateway gateways/gateway
COPY gateways/gateway_echo gateways/gateway_echo
COPY gateways/gateway_host gateways/gateway_host

# Install gateway_host and all its dependencies
RUN uv sync --no-dev --package gateway-host

ENTRYPOINT ["uv", "run", "--no-dev", "--package", "gateway-host", "-m", "gateway_host.api_main"]
