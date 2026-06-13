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
  just client-service-rs-check
  just agent-host-rs-check
  pnpm --dir clients/webui run lint
  pnpm --dir clients/webui run --if-present test
  pnpm --dir clients/webui run build

test:
  uv run --all-packages pytest
  cargo test --quiet --manifest-path services/client_service_rs/Cargo.toml
  cargo test --quiet --manifest-path services/agent_host_rs/Cargo.toml

client-service-rs-check:
  cargo fmt --check --manifest-path services/client_service_rs/Cargo.toml
  cargo test --quiet --manifest-path services/client_service_rs/Cargo.toml
  cargo clippy --manifest-path services/client_service_rs/Cargo.toml --all-targets --all-features

agent-host-rs-check:
  cargo fmt --check --manifest-path services/agent_host_rs/Cargo.toml
  cargo test --quiet --manifest-path services/agent_host_rs/Cargo.toml
  cargo clippy --manifest-path services/agent_host_rs/Cargo.toml --all-targets --all-features

client-service-rs-image:
  runtime="${CONTAINER_RUNTIME:-podman}"; command -v "$runtime" >/dev/null 2>&1 || runtime=docker; "$runtime" build -f services/client_service_rs/Dockerfile -t agentspace-client-service-rs:latest services/client_service_rs

agent-host-rs-image:
  runtime="${CONTAINER_RUNTIME:-podman}"; command -v "$runtime" >/dev/null 2>&1 || runtime=docker; "$runtime" build -f services/agent_host_rs/Dockerfile -t agentspace-agent-host-agent-host:latest services/agent_host_rs

webui-outdated:
  pnpm --dir clients/webui outdated

# Static analysis for the webui (knip: unused/unlisted deps, dead exports)
webui-lint:
  pnpm --dir clients/webui run lint

# Full stack compose workflow
stack-build:
  podman compose -f compose.yaml build

stack-up:
  podman compose -f compose.yaml up -d --build

# Same as stack-up but with the rootless-Podman override (uses the user's
# podman.sock instead of /var/run/docker.sock and works around libpod's
# strict depends_on validation). Requires `systemctl --user enable --now
# podman.socket` once.
stack-up-podman:
  podman compose -f compose.yaml -f compose.podman.yaml up -d --build

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
