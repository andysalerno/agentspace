#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
COMPOSE_FILE="$SCRIPT_DIR/compose.yaml"
PROJECT_NAME="agentspace-client-service"

if [[ ! -f "$SCRIPT_DIR/.env" ]]; then
    cp "$SCRIPT_DIR/.env.example" "$SCRIPT_DIR/.env"
fi

case "${1:-start}" in
    start)
        docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" down --remove-orphans
        docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" build
        docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" up -d
        ;;
    stop)
        docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" down --remove-orphans
        ;;
    logs)
        docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" logs -f
        ;;
    status)
        docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" ps
        ;;
    *)
        echo "Usage: run-service.sh [start|stop|logs|status]" >&2
        exit 1
        ;;
esac
