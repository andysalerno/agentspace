set shell := ["bash", "-cu"]
set windows-shell := ["bash", "-cu"]

kernel_host_script := "kernels/kernel_host/spawn-kernel.sh"
dev_container := "agentspace-dev"
dev_image := "localhost/agentspace-dev:latest"
dev_home_volume := "agentspace-dev-home"

# Show available recipes.
default:
  @just --list

# Install all development dependencies.
bootstrap: bootstrap-python bootstrap-node

# Install Python dependencies.
bootstrap-python:
  uv sync --all-packages --dev

# Install Node.js dependencies.
bootstrap-node:
  cd clients/webui && pnpm install

# Build the openSUSE development image with Podman.
[group('dev')]
build-image-dev:
  podman build --file dev.Dockerfile --tag {{dev_image}} .

# Start the development container in the background.
[group('dev')]
dev-start home="":
  #!/usr/bin/env bash
  set -euo pipefail
  uid="$(id -u)"
  gid="$(id -g)"
  user="$uid:$gid"
  if [[ -n "${PODMAN_SOCKET:-}" ]]; then
    podman_socket="$PODMAN_SOCKET"
  else
    podman_socket="$(
      podman info --format '{{ "{{.Host.RemoteSocket.Path}}" }}'
    )"
  fi
  podman_socket="${podman_socket#unix://}"
  if [[ ! -S "$podman_socket" ]]; then
    echo "Podman socket not found at $podman_socket." >&2
    echo "Start it with: systemctl --user enable --now podman.socket" >&2
    echo "Or set PODMAN_SOCKET to the host socket path." >&2
    exit 1
  fi
  home_dir={{quote(home)}}
  if [[ -n "$home_dir" ]]; then
    mkdir -p -- "$home_dir"
    home_dir="$(cd "$home_dir" && pwd -P)"
    home_key="bind:$home_dir"
    home_volume=("$home_dir:/home/dev:rw")
  else
    home_key="volume:{{dev_home_volume}}"
    home_volume=("{{dev_home_volume}}:/home/dev:U")
  fi
  if ! podman image exists {{dev_image}}; then
    echo "Development image not found; building {{dev_image}}."
    just --justfile "{{justfile_directory()}}/justfile" build-image-dev
  fi
  image_id="$(
    podman image inspect {{dev_image}} \
      --format '{{ "{{.Id}}" }}'
  )"
  if podman container exists {{dev_container}}; then
    container_user="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.Config.User}}" }}'
    )"
    container_home="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.Config.Labels.agentspace_dev_home}}" }}'
    )"
    container_podman_socket="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.Config.Labels.agentspace_dev_podman_socket}}" }}'
    )"
    container_workspace="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.Config.Labels.agentspace_dev_workspace}}" }}'
    )"
    container_image_id="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.Config.Labels.agentspace_dev_image_id}}" }}'
    )"
    if [[ "$container_user" != "$user" \
      || "$container_home" != "$home_key" \
      || "$container_podman_socket" != "$podman_socket" \
      || "$container_workspace" != "{{justfile_directory()}}" \
      || "$container_image_id" != "$image_id" ]]; then
      echo "Existing container configuration changed; recreating {{dev_container}}."
      podman rm --force {{dev_container}} >/dev/null
    fi
  fi
  if podman container exists {{dev_container}}; then
    container_running="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.State.Running}}" }}'
    )"
    if [[ "$container_running" == true ]]; then
      echo "Development container {{dev_container}} is already running."
    else
      echo "Starting existing development container {{dev_container}}."
      podman start {{dev_container}} >/dev/null
    fi
  else
    if [[ -n "$home_dir" ]]; then
      echo "Creating development container {{dev_container}} with home $home_dir."
    else
      echo "Creating development container {{dev_container}} with a persistent home volume."
    fi
    podman run \
      --detach \
      --name {{dev_container}} \
      --hostname agentspace-dev \
      --label agentspace_dev_home="$home_key" \
      --label agentspace_dev_image_id="$image_id" \
      --label agentspace_dev_podman_socket="$podman_socket" \
      --label agentspace_dev_workspace="{{justfile_directory()}}" \
      --userns keep-id \
      --user "$user" \
      --passwd-entry "dev:x:$uid:$gid:Development User:/home/dev:/bin/bash" \
      --env AGENTSPACE_HOST_WORKSPACE="{{justfile_directory()}}" \
      --env AGENTSPACE_PODMAN_SOCKET_PATH="$podman_socket" \
      --env CONTAINER_HOST=unix:///run/podman/podman.sock \
      --env DOCKER_HOST=unix:///run/podman/podman.sock \
      --env HOME=/home/dev \
      --env USER=dev \
      --env LOGNAME=dev \
      --security-opt label=disable \
      --volume "${home_volume[0]}" \
      --volume "$podman_socket:/run/podman/podman.sock:rw" \
      --volume "{{justfile_directory()}}:/workspace:rw" \
      --workdir /workspace \
      {{dev_image}} \
      >/dev/null
    echo "Follow the VS Code tunnel login and status with:"
    echo "  podman logs --follow {{dev_container}}"
  fi

# Stop the development container without removing it.
[group('dev')]
dev-stop:
  #!/usr/bin/env bash
  set -euo pipefail
  if ! podman container exists {{dev_container}}; then
    echo "Development container {{dev_container}} does not exist."
    exit 0
  fi
  container_running="$(
    podman inspect {{dev_container}} \
      --format '{{ "{{.State.Running}}" }}'
  )"
  if [[ "$container_running" == true ]]; then
    echo "Stopping development container {{dev_container}}."
    podman stop {{dev_container}} >/dev/null
  else
    echo "Development container {{dev_container}} is already stopped."
  fi

# Remove the development container, keeping its persistent home volume.
[group('dev')]
dev-remove:
  #!/usr/bin/env bash
  set -euo pipefail
  if ! podman container exists {{dev_container}}; then
    echo "Development container {{dev_container}} does not exist."
    exit 0
  fi
  echo "Removing development container {{dev_container}}."
  podman rm --force {{dev_container}} >/dev/null

# Recreate the development container from scratch, keeping its home volume.
[group('dev')]
dev-restart home="":
  #!/usr/bin/env bash
  set -euo pipefail
  just --justfile "{{justfile_directory()}}/justfile" dev-remove
  just --justfile "{{justfile_directory()}}/justfile" dev-start {{quote(home)}}

# List development containers created by this repository.
[group('dev')]
dev-list-containers:
  @podman ps --all --filter "name={{dev_container}}"

# Remove the development container and its persistent named volumes.
[group('dev')]
dev-clear-volumes:
  #!/usr/bin/env bash
  set -euo pipefail
  if podman container exists {{dev_container}}; then
    podman rm --force {{dev_container}} >/dev/null
  fi
  if podman volume exists {{dev_home_volume}}; then
    podman volume rm {{dev_home_volume}} >/dev/null
  fi

# Enter an interactive Bash shell in the development container.
[group('dev')]
dev-shell home="":
  #!/usr/bin/env bash
  set -euo pipefail
  home_dir={{quote(home)}}
  if [[ -n "$home_dir" ]]; then
    just --justfile "{{justfile_directory()}}/justfile" dev-start "$home_dir"
  elif ! podman container exists {{dev_container}}; then
    just --justfile "{{justfile_directory()}}/justfile" dev-start
  else
    container_running="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.State.Running}}" }}'
    )"
    if [[ "$container_running" == true ]]; then
      echo "Attaching to running development container {{dev_container}}."
    else
      echo "Starting existing development container {{dev_container}} before attaching."
      podman start {{dev_container}} >/dev/null
    fi
  fi
  echo "Opening an interactive shell in {{dev_container}}."
  exec podman exec \
    --interactive \
    --tty \
    --env TERM \
    --env COLORTERM \
    --workdir /workspace \
    {{dev_container}} \
    /bin/bash

# Run the full repository verification suite.
[group('check')]
check: check-rust check-python check-js

# Check the Rust workspace with fmt, tests, and Clippy.
[group('check')]
check-rust:
  cargo fmt --check --all
  cargo test --quiet --workspace
  cargo clippy --workspace --all-targets --all-features

# Check Python formatting, linting, types, and tests.
[group('check')]
check-python:
  just bootstrap-python
  uv run ruff format --check .
  uv run ruff check .
  uv run pyright
  uv run --all-packages pytest

# Check JavaScript linting, tests, and the production build.
[group('check')]
check-js:
  just bootstrap-node
  cd clients/webui && pnpm run lint
  cd clients/webui && pnpm run --if-present test
  cd clients/webui && pnpm run build

# Run all repository tests.
[group('check')]
test: test-rust test-python test-js

# Run Rust workspace tests.
[group('check')]
test-rust:
  cargo test --quiet --workspace

# Run all Python package tests.
[group('check')]
test-python:
  uv run --all-packages pytest

# Run JavaScript tests when configured.
[group('check')]
test-js:
  cd clients/webui && pnpm run --if-present test

# Check only the client-service Rust crate.
[group('check')]
client-service-check:
  cargo fmt --check --manifest-path services/client_service_rs/Cargo.toml
  cargo test --quiet --manifest-path services/client_service_rs/Cargo.toml
  cargo clippy --manifest-path services/client_service_rs/Cargo.toml --all-targets --all-features

# Check only the agent-host Rust crate.
[group('check')]
agent-host-check:
  cargo fmt --check --manifest-path services/agent_host_rs/Cargo.toml
  cargo test --quiet --manifest-path services/agent_host_rs/Cargo.toml
  cargo clippy --manifest-path services/agent_host_rs/Cargo.toml --all-targets --all-features

# Build the client-service container image.
[group('build')]
build-image-client-service:
  #!/usr/bin/env bash
  set -euo pipefail
  runtime="${CONTAINER_RUNTIME:-podman}"
  if ! command -v "$runtime" >/dev/null 2>&1; then
    runtime=docker
  fi
  rust_profile="${RUST_BUILD_PROFILE:-debug}"
  version="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  "$runtime" build \
    --build-arg "AGENTSPACE_VERSION=$version" \
    --build-arg "RUST_BUILD_PROFILE=$rust_profile" \
    --file services/client_service_rs/Dockerfile \
    --tag agentspace-client-service:latest \
    .

# Build the agent-host container image.
[group('build')]
build-image-agent-host:
  #!/usr/bin/env bash
  set -euo pipefail
  runtime="${CONTAINER_RUNTIME:-podman}"
  if ! command -v "$runtime" >/dev/null 2>&1; then
    runtime=docker
  fi
  rust_profile="${RUST_BUILD_PROFILE:-debug}"
  version="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  "$runtime" build \
    --build-arg "AGENTSPACE_VERSION=$version" \
    --build-arg "RUST_BUILD_PROFILE=$rust_profile" \
    --file services/agent_host_rs/Dockerfile \
    --tag agentspace-agent-host:latest \
    .

# Run webui ESLint and dependency/dead-code checks.
[group('check')]
webui-lint:
  cd clients/webui && pnpm run lint

# Check for outdated webui dependencies.
[group('check')]
webui-deps-outdated:
  cd clients/webui && pnpm outdated

# Install the screenshot harness dependencies and the Playwright browser.
[group('dev')]
webui-screenshots-setup:
  cd clients/webui && pnpm install
  cd tools/webui-screenshots && pnpm install
  cd tools/webui-screenshots && ./node_modules/.bin/playwright install chromium

# Screenshot every webui view against the mock API. See docs/PLAYWRIGHT.md.
[group('dev')]
webui-screenshots out="tools/webui-screenshots/out":
  #!/usr/bin/env bash
  set -euo pipefail
  cd clients/webui && pnpm run build
  cd "{{justfile_directory()}}/tools/webui-screenshots"
  node mock-api.mjs &
  server=$!
  trap 'kill "$server" 2>/dev/null || true' EXIT
  for _ in $(seq 1 20); do
    if ! kill -0 "$server" 2>/dev/null; then
      echo "mock-api exited before serving; is port 8010 already in use?" >&2
      exit 1
    fi
    if curl -sf http://127.0.0.1:8010/info.json >/dev/null; then break; fi
    sleep 0.5
  done
  node capture.mjs "{{justfile_directory()}}/{{out}}"

[private]
_stack-runtime:
  #!/usr/bin/env bash
  set -euo pipefail
  if [[ -n "${CONTAINER_RUNTIME:-}" ]]; then
    echo "$CONTAINER_RUNTIME"
  elif podman info >/dev/null 2>&1; then
    echo podman
  elif docker info >/dev/null 2>&1; then
    echo docker
  else
    echo "Cannot connect to Podman or Docker." >&2
    exit 1
  fi

[private]
_stack-compose *args:
  #!/usr/bin/env bash
  set -euo pipefail
  runtime="$(just --justfile "{{justfile_directory()}}/justfile" _stack-runtime)"
  compose_files=(--file compose.yaml)
  args=({{args}})
  if [[ "$runtime" == podman ]]; then
    export AGENTSPACE_HOST_WORKSPACE="${AGENTSPACE_HOST_WORKSPACE:-{{justfile_directory()}}}"
    socket="${AGENTSPACE_PODMAN_SOCKET_PATH:-$(
      podman info --format '{{ "{{.Host.RemoteSocket.Path}}" }}'
    )}"
    export AGENTSPACE_PODMAN_SOCKET_PATH="${socket#unix://}"
    compose_files+=(--file compose.podman.yaml)
  fi
  "$runtime" compose "${compose_files[@]}" "${args[@]}"

# Build all stack container images with Compose.
[group('run')]
build-image-stack:
  #!/usr/bin/env bash
  set -euo pipefail
  export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  just --justfile "{{justfile_directory()}}/justfile" _stack-compose build

# Build stack container images with Compose without using cached layers.
[group('build')]
build-image-stack-no-cache *services:
  #!/usr/bin/env bash
  set -euo pipefail
  export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  just --justfile "{{justfile_directory()}}/justfile" _stack-compose build \
    --no-cache {{services}}

# Start the full Compose stack with an available Podman or Docker runtime.
[group('run')]
stack-up:
  #!/usr/bin/env bash
  set -euo pipefail
  export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  just --justfile "{{justfile_directory()}}/justfile" _stack-compose up --detach

# Start the stack with Podman. Prefer `stack-up`, which selects the runtime.
[group('run')]
stack-up-rootless-podman:
  CONTAINER_RUNTIME=podman just --justfile "{{justfile_directory()}}/justfile" stack-up

[private]
_stack-up-force-recreate *services:
  just --justfile "{{justfile_directory()}}/justfile" _stack-compose up --detach --force-recreate {{services}}

# Rebuild rootless Podman stack images without cache, then recreate containers.
[group('run')]
stack-rebuild-rootless-podman *services:
  CONTAINER_RUNTIME=podman just --justfile "{{justfile_directory()}}/justfile" build-image-stack-no-cache {{services}}
  CONTAINER_RUNTIME=podman just --justfile "{{justfile_directory()}}/justfile" _stack-up-force-recreate {{services}}

# Stop the Compose stack and clean spawned kernel/gateway containers.
[group('run')]
stack-down:
  #!/usr/bin/env bash
  set -euo pipefail
  cleanup_url="http://127.0.0.1:${CONTAINER_CLIENT_SERVICE_PORT:-8002}/management/runtime-cleanup"
  if ! curl --fail --silent --show-error \
    --header "content-type: application/json" \
    --data '{"dry_run":false}' \
    "$cleanup_url" >/dev/null; then
    echo "Warning: runtime orphan cleanup was unavailable; managed session volumes were retained." >&2
  fi
  just --justfile "{{justfile_directory()}}/justfile" _stack-compose down --remove-orphans
  runtime="$(just --justfile "{{justfile_directory()}}/justfile" _stack-runtime)"
  mapfile -t spawned_containers < <(
    {
      "$runtime" ps -aq --filter "label=agentspace.role=kernel"
      "$runtime" ps -aq --filter "label=agentspace.role=gateway"
    } | sort -u
  )
  if (( ${#spawned_containers[@]} > 0 )); then
    "$runtime" rm -f "${spawned_containers[@]}" >/dev/null
  fi

# Tail logs for the full Compose stack.
[group('run')]
stack-logs:
  just --justfile "{{justfile_directory()}}/justfile" _stack-compose logs --follow

# Show Compose service status.
[group('run')]
stack-status:
  just --justfile "{{justfile_directory()}}/justfile" _stack-compose ps

# Run the containerized memory release smoke flow against prebuilt images.
[group('check')]
memory-e2e:
  CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-podman}" bash scripts/memory-e2e.sh
