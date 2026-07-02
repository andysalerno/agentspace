# About AgentSpace

AgentSpace is a system for defining, interacting with, and observing AI agents.

## System Architecture

The repository is a monorepo managed by `uv` for Python packages, with Rust
services managed by the Cargo workspace at the repository root.

### Key Components

- **Kernels** (`kernels/`): Shared protocol and event schemas.
  - `kernel_copilot`: Primary kernel path (uses `copilot-cli`).
  - `kernel_host`: Runner for kernel containers.
- **Services** (`services/`):
  - `agent_host` (`services/agent_host_rs`): Manages sessions by spawning `kernel_host` containers.
  - `client_service` (`services/client_service_rs`): Public backend API. Clients should talk to this, not `agent_host` directly.
- **Clients** (`clients/`, `channels/`):
  - `webui`: TypeScript dashboard.
  - `cli_channel`: CLI session client for validating `client_service` contract.

## Development Workflow

### Commands

Use `just` for common tasks:
- `just bootstrap`: Install all dependencies (`uv sync` and `pnpm install`).
- `just check`: Run the full repo verification suite before finishing work.
- `just test`: Run Python and service tests.
- `just stack-up`: Start the full stack using Docker Compose.
- `just stack-down`: Stop the full stack.
- `just copilot-setup`: Run `kernels/kernel_host/spawn-kernel.sh setup` to authenticate Copilot.

Run `just check` after completing any code or documentation change. It covers
Python formatting, linting, type-checking, tests, web linting, pnpm tests when
present, and the web build.

### Python Standards

- **Package Manager**: `uv`
- **Type Checking**: `pyright` (strict mode)
- **Linting/Formatting**: `ruff` (all defaults enabled)
- **Testing**: `pytest` with `asyncio_mode = "auto"`

### Docker & Environment

- **Kernel Host**: Requires `KERNEL_WORKDIR` to be set in `.env` or `kernels/kernel_host/.env.example` (not defaulted by compose).
- **Copilot Sessions**: To resume a session, set `COPILOT_SESSION_ID` in `kernels/kernel_host/.env.example`.
- **Service Ports**:
- `agent_host`: `http://127.0.0.1:8001`
- `client_service`: `http://127.0.0.1:8002`
- `webui`: `http://127.0.0.1:8003`

## Git Management

This repo lives in github at: https://github.com/andysalerno/agentspace

You may use the `gh` cli tool to interact with the repo on github.