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

fmt:
  uv run ruff format .

fmt-check:
  uv run ruff format --check .

lint:
  uv run ruff check .

fix:
  uv run ruff check --fix .

typecheck:
  uv run pyright

test:
  uv run pytest

test-kernels:
  uv run pytest kernels

test-services:
  uv run pytest services

test-channels:
  uv run pytest channels

check: fmt-check lint typecheck test

# Full stack compose workflow
stack-build:
  docker compose -f compose.yaml build

stack-up:
  docker compose -f compose.yaml up -d --build

stack-down:
  docker compose -f compose.yaml down --remove-orphans

stack-logs:
  docker compose -f compose.yaml logs -f

stack-status:
  docker compose -f compose.yaml ps

# Service wrappers around the repo scripts
agent-host-start:
  bash {{agent_host_script}} start

agent-host-stop:
  bash {{agent_host_script}} stop

agent-host-logs:
  bash {{agent_host_script}} logs

agent-host-status:
  bash {{agent_host_script}} status

client-service-start:
  bash {{client_service_script}} start

client-service-stop:
  bash {{client_service_script}} stop

client-service-logs:
  bash {{client_service_script}} logs

client-service-status:
  bash {{client_service_script}} status

webui-start:
  bash {{webui_script}} start

webui-stop:
  bash {{webui_script}} stop

webui-logs:
  bash {{webui_script}} logs

webui-status:
  bash {{webui_script}} status

# Kernel host helpers
kernel-setup:
  bash {{kernel_host_script}} setup

kernel-run prompt:
  bash {{kernel_host_script}} {{prompt}}

# Local web and channel helpers
webui-build:
  npm --prefix clients/webui run build

cli-new agent_id name="terminal-1":
  uv run --package cli-channel -m cli_channel --agent-id {{agent_id}} --name {{name}}

cli-resume session_id:
  uv run --package cli-channel -m cli_channel --session-id {{session_id}}
