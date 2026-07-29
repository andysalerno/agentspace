#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"

runtime_available() {
  command -v "$1" >/dev/null 2>&1 && "$1" info >/dev/null 2>&1
}

require_runtime() {
  local runtime="$1"
  if [[ "$runtime" != podman && "$runtime" != docker ]]; then
    echo "CONTAINER_RUNTIME must be 'podman' or 'docker', not '$runtime'." >&2
    exit 2
  fi
  if ! runtime_available "$runtime"; then
    echo "Cannot connect to $runtime. Ensure its daemon is running and accessible." >&2
    exit 1
  fi
}

select_runtime() {
  if [[ -n "${CONTAINER_RUNTIME:-}" ]]; then
    require_runtime "$CONTAINER_RUNTIME"
    printf '%s\n' "$CONTAINER_RUNTIME"
    return
  fi

  if [[ -n "${CONTAINER_HOST:-}" ]]; then
    require_runtime podman
    printf 'podman\n'
    return
  fi

  if [[ -n "${DOCKER_HOST:-}" ]]; then
    if [[ "$DOCKER_HOST" == *podman.sock* ]]; then
      require_runtime podman
      printf 'podman\n'
    else
      require_runtime docker
      printf 'docker\n'
    fi
    return
  fi

  if runtime_available podman; then
    printf 'podman\n'
  elif runtime_available docker; then
    printf 'docker\n'
  else
    echo "Cannot connect to Podman or Docker. Ensure one daemon is running and accessible." >&2
    exit 1
  fi
}

runtime="$(select_runtime)"
action="${1:-}"
if [[ -z "$action" ]]; then
  echo "Usage: $0 <compose|cleanup> [arguments...]" >&2
  exit 2
fi
shift

case "$action" in
  compose)
    compose_files=(--file "$repo_dir/compose.yaml")
    if [[ "$runtime" == podman ]]; then
      compose_command="${1:-}"
      if [[ "$compose_command" =~ ^(create|run|up)$ \
        && -z "${AGENTSPACE_HOST_WORKSPACE:-}" \
        && -e /run/.containerenv ]]; then
        echo "The container is missing its host workspace path." >&2
        echo "Recreate it from the host with: just dev-start" >&2
        exit 1
      fi
      export AGENTSPACE_HOST_WORKSPACE="${AGENTSPACE_HOST_WORKSPACE:-$repo_dir}"
      if [[ -z "${AGENTSPACE_PODMAN_SOCKET_PATH:-}" ]]; then
        AGENTSPACE_PODMAN_SOCKET_PATH="$(
          "$runtime" info --format '{{.Host.RemoteSocket.Path}}'
        )"
      fi
      export AGENTSPACE_PODMAN_SOCKET_PATH="${AGENTSPACE_PODMAN_SOCKET_PATH#unix://}"
      compose_files+=(--file "$repo_dir/compose.podman.yaml")
    fi
    exec "$runtime" compose "${compose_files[@]}" "$@"
    ;;
  cleanup)
    for role in kernel gateway; do
      mapfile -t container_ids < <(
        "$runtime" ps -q --filter "label=agentspace.role=$role"
      )
      if (( ${#container_ids[@]} > 0 )); then
        "$runtime" rm -f "${container_ids[@]}"
      fi
    done
    ;;
  *)
    echo "Unknown action '$action'. Expected 'compose' or 'cleanup'." >&2
    exit 2
    ;;
esac
