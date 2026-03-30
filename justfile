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

# Full stack compose workflow
stack-build:
  docker compose -f compose.yaml build

stack-up:
  docker compose -f compose.yaml up -d --build

stack-down:
  docker compose -f compose.yaml down --remove-orphans
  -docker rm -f $(docker ps -q --filter "label=agentspace.role=kernel") 2>/dev/null || true

stack-logs:
  docker compose -f compose.yaml logs -f

stack-status:
  docker compose -f compose.yaml ps

# One-time setup: launch interactive copilot session for /login auth
copilot-setup:
  {{kernel_host_script}} setup
