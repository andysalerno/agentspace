set shell := ["bash", "-cu"]
set windows-shell := ["bash", "-cu"]

kernel_host_script := "kernels/kernel_host/spawn-kernel.sh"

# Show available recipes.
default:
  @just --list

# Install Python and web dependencies.
bootstrap:
  uv sync --all-packages --dev
  pnpm --dir clients/webui install

# Run the full repository verification suite.
[group('check')]
check:
  uv run ruff format --check .
  uv run ruff check .
  uv run pyright
  uv run --all-packages pytest
  just rust-check
  pnpm --dir clients/webui run lint
  pnpm --dir clients/webui run --if-present test
  pnpm --dir clients/webui run build

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

# Check only the session-tools Rust crate.
[group('check')]
session-tools-check:
  cargo fmt --check --manifest-path services/session_tools_rs/Cargo.toml
  cargo test --quiet --manifest-path services/session_tools_rs/Cargo.toml
  cargo clippy --manifest-path services/session_tools_rs/Cargo.toml --all-targets --all-features

# Build the client-service container image.
[group('build')]
client-service-build-image:
  runtime="${CONTAINER_RUNTIME:-podman}"; command -v "$runtime" >/dev/null 2>&1 || runtime=docker; rust_profile="${RUST_BUILD_PROFILE:-debug}"; version="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"; "$runtime" build --build-arg "AGENTSPACE_VERSION=$version" --build-arg "RUST_BUILD_PROFILE=$rust_profile" -f services/client_service_rs/Dockerfile -t agentspace-client-service:latest .

# Build the agent-host container image.
[group('build')]
agent-host-build-image:
  runtime="${CONTAINER_RUNTIME:-podman}"; command -v "$runtime" >/dev/null 2>&1 || runtime=docker; rust_profile="${RUST_BUILD_PROFILE:-debug}"; version="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"; "$runtime" build --build-arg "AGENTSPACE_VERSION=$version" --build-arg "RUST_BUILD_PROFILE=$rust_profile" -f services/agent_host_rs/Dockerfile -t agentspace-agent-host:latest .

# Run webui ESLint and dependency/dead-code checks.
[group('check')]
webui-lint:
  pnpm --dir clients/webui run lint

# Check for outdated webui dependencies.
[group('check')]
webui-deps-outdated:
  pnpm --dir clients/webui outdated

# Build all stack container images with Compose.
[group('build')]
stack-build-images:
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml build

# Build stack container images with Compose without using cached layers.
[group('build')]
stack-build-images-no-cache *services:
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml build --no-cache {{services}}

# Start the full Compose stack.
[group('run')]
stack-up:
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml up -d

# Start the stack with the rootless Podman override.
[group('run')]
stack-up-rootless-podman:
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml -f compose.podman.yaml up -d

# Rebuild rootless Podman stack images without cache, then recreate containers.
[group('run')]
stack-rebuild-rootless-podman *services:
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml -f compose.podman.yaml build --no-cache {{services}}
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml -f compose.podman.yaml up -d --force-recreate {{services}}

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
