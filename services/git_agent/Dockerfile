# syntax=docker/dockerfile:1

FROM docker.io/library/python:3.13-slim

ARG AGENTSPACE_VERSION=dev

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}
ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates git && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY pyproject.toml uv.lock ./
COPY services/git_agent services/git_agent

RUN --mount=type=cache,id=agentspace-uv-cache,target=/root/.cache/uv,sharing=locked \
    uv sync --frozen --no-dev --package git-agent

EXPOSE 8004

ENTRYPOINT ["uv", "run", "--no-dev", "--package", "git-agent", "-m", "git_agent"]
