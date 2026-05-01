FROM docker.io/library/python:3.13-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

ENV UV_COMPILE_BYTECODE=1
ENV UV_LINK_MODE=copy

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates git && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY pyproject.toml pyproject.toml
COPY services/git_agent services/git_agent

RUN uv sync --no-dev --package git-agent

EXPOSE 8004

ENTRYPOINT ["uv", "run", "--no-dev", "--package", "git-agent", "-m", "git_agent"]
