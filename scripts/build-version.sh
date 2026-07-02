#!/usr/bin/env bash
set -euo pipefail

timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
sha="$(git rev-parse --short=12 HEAD 2>/dev/null || true)"

if [[ -z "$sha" ]]; then
    sha="unknown"
fi

printf '%s-%s\n' "$sha" "$timestamp"
