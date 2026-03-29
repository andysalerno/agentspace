FROM python:3.13-slim

COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/

WORKDIR /app

COPY pyproject.toml pyproject.toml
COPY clients/webui clients/webui

RUN uv sync --no-dev --package webui

EXPOSE 8003

CMD ["/usr/local/bin/uv", "run", "--no-dev", "--package", "webui", "-m", "webui"]
