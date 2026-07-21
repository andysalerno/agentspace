#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
COMPOSE_FILE="$SCRIPT_DIR/compose.copilot.yaml"
PROJECT_NAME="agentspace-kernel"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Ensure .env exists (compose requires it via env_file)
if [[ ! -f "$SCRIPT_DIR/.env" ]]; then
    cp "$SCRIPT_DIR/.env.example" "$SCRIPT_DIR/.env"
fi

export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash "$REPO_ROOT/scripts/build-version.sh")}"

cleanup() {
    docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" down --remove-orphans >/dev/null 2>&1 || true
}

cleanup
trap cleanup EXIT

echo "Building kernel image..." >&2
docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" build kernel >&2

MESSAGE="${1:?Usage: spawn-kernel.sh <message>}"
echo "Running kernel..." >&2
docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" run --rm kernel "$MESSAGE"
