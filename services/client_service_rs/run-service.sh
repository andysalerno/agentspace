#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

export CLIENT_SERVICE_HOST="${CLIENT_SERVICE_HOST:-0.0.0.0}"
export CLIENT_SERVICE_PORT="${CLIENT_SERVICE_PORT:-8002}"

cd "$SCRIPT_DIR"
exec cargo run
