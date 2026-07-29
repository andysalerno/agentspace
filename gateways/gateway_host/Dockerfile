# syntax=docker/dockerfile:1

FROM docker.io/library/python:3.14-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy

WORKDIR /app

# Copy all gateway packages and root project config
COPY pyproject.toml uv.lock ./
COPY gateways/gateway gateways/gateway
COPY gateways/gateway_echo gateways/gateway_echo
COPY gateways/gateway_discord gateways/gateway_discord
COPY gateways/gateway_host gateways/gateway_host

# Install gateway_host and all its dependencies
RUN --mount=type=cache,id=agentspace-uv-cache,target=/root/.cache/uv,sharing=locked \
    uv sync --frozen --no-dev --package gateway-host

ENTRYPOINT ["uv", "run", "--no-dev", "--package", "gateway-host", "-m", "gateway_host.api_main"]

ARG AGENTSPACE_VERSION=dev
ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}
