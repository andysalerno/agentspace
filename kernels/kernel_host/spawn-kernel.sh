#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
COMPOSE_FILE="$SCRIPT_DIR/compose.copilot.yaml"
PROJECT_NAME="agentspace-kernel"

# Ensure .env exists (compose requires it via env_file)
if [[ ! -f "$SCRIPT_DIR/.env" ]]; then
    cp "$SCRIPT_DIR/.env.example" "$SCRIPT_DIR/.env"
fi

cleanup() {
    docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" down --remove-orphans >/dev/null 2>&1 || true
}

cleanup
trap cleanup EXIT

echo "Building kernel image..." >&2
docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" build kernel setup >&2

if [[ "${1:-}" == "setup" ]]; then
    echo "Starting interactive copilot session (run /login to authenticate)..." >&2
    docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" run --rm setup
else
    MESSAGE="${1:?Usage: spawn-kernel.sh <message|setup>}"
    echo "Running kernel..." >&2
    docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" run --rm kernel "$MESSAGE"
fi
