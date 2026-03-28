#!/usr/bin/env bash
set -euo pipefail

MESSAGE="${1:?Usage: spawn-kernel.sh <message>}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Use .env if it exists, otherwise fall back to .env.example
ENV_FILE="$SCRIPT_DIR/.env"
if [[ ! -f "$ENV_FILE" ]]; then
    ENV_FILE="$SCRIPT_DIR/.env.example"
fi

echo "Building kernel image..." >&2
docker build -t agentspace-kernel -f "$SCRIPT_DIR/Dockerfile" "$REPO_ROOT"

echo "Running kernel..." >&2
docker run --rm --env-file "$ENV_FILE" agentspace-kernel "$MESSAGE"
