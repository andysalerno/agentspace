set shell := ["bash", "-cu"]
set windows-shell := ["bash", "-cu"]

agent_host_script := "services/agent_host/run-service.sh"
client_service_script := "services/client_service/run-service.sh"
webui_script := "clients/webui/run-service.sh"
kernel_host_script := "kernels/kernel_host/spawn-kernel.sh"

default:
  @just --list

help:
  @just --list

# Workspace setup and validation
bootstrap:
  uv sync --all-packages --dev
  npm --prefix clients/webui install

test:
  uv run --all-packages pytest

webui-outdated:
  npm --prefix clients/webui outdated

# Static analysis for the webui (knip: unused/unlisted deps, dead exports)
webui-lint:
  npm --prefix clients/webui run lint

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
