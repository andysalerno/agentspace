#!/usr/bin/env bash

set -euo pipefail

target="${1:-}"

sync_python() {
    uv sync --all-packages --dev --locked
}

sync_webui() {
    pnpm --dir clients/webui install --frozen-lockfile
}

case "$target" in
    check)
        sync_python
        sync_webui
        just check
        ;;
    test)
        sync_python
        just test
        ;;
    rust-check | client-service-check | agent-host-check)
        just "$target"
        ;;
    webui-lint)
        sync_webui
        just webui-lint
        ;;
    webui-build)
        sync_webui
        pnpm --dir clients/webui run build
        ;;
    *)
        echo "unsupported containerized check target: $target" >&2
        exit 2
        ;;
esac
