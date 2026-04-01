#!/usr/bin/env bash
set -euo pipefail

export ANTHROPIC_BASE_URL=http://nzxt.local:8000
export ANTHROPIC_AUTH_TOKEN=empty
export ANTHROPIC_MODEL=opus
export ANTHROPIC_DEFAULT_OPUS_MODEL=model
export ANTHROPIC_DEFAULT_SONNET_MODEL=model
export ANTHROPIC_DEFAULT_HAIKU_MODEL=model
exec claude "$@"