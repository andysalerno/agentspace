set shell := ["bash", "-cu"]
set windows-shell := ["bash", "-cu"]

agent_host_script := "services/agent_host_rs/run-service.sh"
client_service_script := "services/client_service_rs/run-service.sh"
webui_script := "clients/webui/run-service.sh"
kernel_host_script := "kernels/kernel_host/spawn-kernel.sh"

default:
  @just --list

help:
  @just --list

# Workspace setup and validation
bootstrap:
  uv sync --all-packages --dev
  pnpm --dir clients/webui install

check:
  uv run ruff format --check .
  uv run ruff check .
  uv run pyright
  uv run --all-packages pytest
  just rust-check
  pnpm --dir clients/webui run lint
  pnpm --dir clients/webui run --if-present test
  pnpm --dir clients/webui run build

test:
  uv run --all-packages pytest
  cargo test --quiet --workspace

rust-check:
  cargo fmt --check --all
  cargo test --quiet --workspace
  cargo clippy --workspace --all-targets --all-features

client-service-check:
  cargo fmt --check --manifest-path services/client_service_rs/Cargo.toml
  cargo test --quiet --manifest-path services/client_service_rs/Cargo.toml
  cargo clippy --manifest-path services/client_service_rs/Cargo.toml --all-targets --all-features

agent-host-check:
  cargo fmt --check --manifest-path services/agent_host_rs/Cargo.toml
  cargo test --quiet --manifest-path services/agent_host_rs/Cargo.toml
  cargo clippy --manifest-path services/agent_host_rs/Cargo.toml --all-targets --all-features

client-service-image:
  runtime="${CONTAINER_RUNTIME:-podman}"; command -v "$runtime" >/dev/null 2>&1 || runtime=docker; rust_profile="${RUST_BUILD_PROFILE:-debug}"; version="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"; "$runtime" build --build-arg "AGENTSPACE_VERSION=$version" --build-arg "RUST_BUILD_PROFILE=$rust_profile" -f services/client_service_rs/Dockerfile -t agentspace-client-service:latest .

agent-host-image:
  runtime="${CONTAINER_RUNTIME:-podman}"; command -v "$runtime" >/dev/null 2>&1 || runtime=docker; rust_profile="${RUST_BUILD_PROFILE:-debug}"; version="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}"; "$runtime" build --build-arg "AGENTSPACE_VERSION=$version" --build-arg "RUST_BUILD_PROFILE=$rust_profile" -f services/agent_host_rs/Dockerfile -t agentspace-agent-host:latest .

webui-outdated:
  pnpm --dir clients/webui outdated

# Static analysis for the webui (knip: unused/unlisted deps, dead exports)
webui-lint:
  pnpm --dir clients/webui run lint

# Full stack compose workflow
stack-build:
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml build

stack-up:
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml up -d

# Same as stack-up but with the rootless-Podman override (uses the user's
# podman.sock instead of /var/run/docker.sock and works around libpod's
# strict depends_on validation). Requires `systemctl --user enable --now
# podman.socket` once.
stack-up-podman:
  AGENTSPACE_VERSION="${AGENTSPACE_VERSION:-$(bash scripts/build-version.sh)}" podman compose -f compose.yaml -f compose.podman.yaml up -d --build

stack-down:
  podman compose -f compose.yaml down --remove-orphans
  -podman rm -f $(podman ps -q --filter "label=agentspace.role=kernel") 2>/dev/null || true
  -podman rm -f $(podman ps -q --filter "label=agentspace.role=gateway") 2>/dev/null || true

stack-logs:
  podman compose -f compose.yaml logs -f

stack-status:
  podman compose -f compose.yaml ps

# One-time setup: launch interactive copilot session for /login auth
copilot-setup:
  {{kernel_host_script}} setup
