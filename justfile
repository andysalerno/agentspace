set shell := ["bash", "-cu"]
set windows-shell := ["bash", "-cu"]

kernel_host_script := "kernels/kernel_host/spawn-kernel.sh"
dev_container := "agentspace-dev"
dev_image := "localhost/agentspace-dev:latest"
dev_cargo_volume := "agentspace-dev-cargo"
dev_home_volume := "agentspace-dev-home"
dev_node_modules_volume := "agentspace-dev-node-modules"
dev_target_volume := "agentspace-dev-target"
dev_venv_volume := "agentspace-dev-venv"

# Show available recipes.
default:
  @just --list

# Install Python and web dependencies.
bootstrap:
  uv sync --all-packages --dev
  cd clients/webui && pnpm install

# Build the openSUSE development image with Podman.
[group('dev')]
dev-build-image:
  podman build --file dev.Dockerfile --tag {{dev_image}} .

# Start the development container in the background.
[group('dev')]
dev-start home="":
  #!/usr/bin/env bash
  set -euo pipefail
  uid="$(id -u)"
  gid="$(id -g)"
  user="$uid:$gid"
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
    just --justfile "{{justfile_directory()}}/justfile" dev-build-image
  fi
  if podman container exists {{dev_container}}; then
    container_user="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.Config.User}}" }}'
    )"
    container_home="$(
      podman inspect {{dev_container}} \
        --format '{{ "{{.Config.Labels.agentspace_dev_home}}" }}'
    )"
    if [[ "$container_user" != "$user" || "$container_home" != "$home_key" ]]; then
      podman rm --force {{dev_container}} >/dev/null
    fi
  fi
  if podman container exists {{dev_container}}; then
    podman start {{dev_container}} >/dev/null
  else
    podman run \
      --detach \
      --name {{dev_container}} \
      --hostname agentspace-dev \
      --label agentspace_dev_home="$home_key" \
      --userns keep-id \
      --user "$user" \
      --passwd-entry "dev:x:$uid:$gid:Development User:/home/dev:/bin/bash" \
      --env HOME=/home/dev \
      --env USER=dev \
      --env LOGNAME=dev \
      --security-opt label=disable \
      --volume {{dev_cargo_volume}}:/opt/cargo:U \
      --volume "${home_volume[0]}" \
      --volume {{dev_node_modules_volume}}:/workspace/clients/webui/node_modules:U \
      --volume {{dev_target_volume}}:/workspace/target:U \
      --volume {{dev_venv_volume}}:/workspace/.venv:U \
      --volume "{{justfile_directory()}}:/workspace:rw" \
      --workdir /workspace \
      {{dev_image}} \
      sleep infinity \
      >/dev/null
  fi

# Remove the development container and its persistent named volumes.
[group('dev')]
dev-clear-volumes:
  #!/usr/bin/env bash
  set -euo pipefail
  if podman container exists {{dev_container}}; then
    podman rm --force {{dev_container}} >/dev/null
  fi
  volumes=(
    {{dev_cargo_volume}}
    {{dev_home_volume}}
    {{dev_node_modules_volume}}
    {{dev_target_volume}}
    {{dev_venv_volume}}
  )
  for volume in "${volumes[@]}"; do
    if podman volume exists "$volume"; then
      podman volume rm "$volume" >/dev/null
    fi
  done

# Enter an interactive Bash shell in the development container.
[group('dev')]
dev-shell home="": (dev-start home)
  #!/usr/bin/env bash
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
check:
  uv run ruff format --check .
  uv run ruff check .
  uv run pyright
  uv run --all-packages pytest
  just rust-check
  cd clients/webui && pnpm run lint
  cd clients/webui && pnpm run --if-present test
  cd clients/webui && pnpm run build

# Run all Python and Rust tests.
[group('check')]
test:
  uv run --all-packages pytest
  cargo test --quiet --workspace

# Check the Rust workspace with fmt, tests, and Clippy.
[group('check')]
rust-check:
  cargo fmt --check --all
  cargo test --quiet --workspace
  cargo clippy --workspace --all-targets --all-features

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
client-service-build-image:
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
agent-host-build-image:
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

# Build all stack container images with Compose.
[group('build')]
stack-build-images:
  #!/usr/bin/env bash
  set -euo pipefail
  export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  podman compose \
    --file compose.yaml \
    build

# Build stack container images with Compose without using cached layers.
[group('build')]
stack-build-images-no-cache *services:
  #!/usr/bin/env bash
  set -euo pipefail
  export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  podman compose \
    --file compose.yaml \
    build \
    --no-cache {{services}}

# Start the full Compose stack.
[group('run')]
stack-up:
  #!/usr/bin/env bash
  set -euo pipefail
  export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  podman compose \
    --file compose.yaml \
    up \
    --detach

# Start the stack with the rootless Podman override.
[group('run')]
stack-up-rootless-podman:
  #!/usr/bin/env bash
  set -euo pipefail
  export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  podman compose \
    --file compose.yaml \
    --file compose.podman.yaml \
    up \
    --detach

# Rebuild rootless Podman stack images without cache, then recreate containers.
[group('run')]
stack-rebuild-rootless-podman *services:
  #!/usr/bin/env bash
  set -euo pipefail
  export AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"
  podman compose \
    --file compose.yaml \
    --file compose.podman.yaml \
    build \
    --no-cache {{services}}
  podman compose \
    --file compose.yaml \
    --file compose.podman.yaml \
    up \
    --detach \
    --force-recreate {{services}}

# Stop the Compose stack and clean spawned kernel/gateway containers.
[group('run')]
stack-down:
  podman compose -f compose.yaml down --remove-orphans
  -podman rm -f $(podman ps -q --filter "label=agentspace.role=kernel") 2>/dev/null || true
  -podman rm -f $(podman ps -q --filter "label=agentspace.role=gateway") 2>/dev/null || true

# Tail logs for the full Compose stack.
[group('run')]
stack-logs:
  podman compose -f compose.yaml logs -f

# Show Compose service status.
[group('run')]
stack-status:
  podman compose -f compose.yaml ps

# Run the containerized memory release smoke flow against prebuilt images.
[group('check')]
memory-e2e:
  CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-podman}" bash scripts/memory-e2e.sh
