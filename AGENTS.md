# About AgentSpace

AgentSpace is a system for defining, interacting with, and observing AI agents.

## System Architecture

The repository is a monorepo managed by `uv` for Python packages, with the
Rust client service managed by Cargo under `services/client_service_rs`.

### Key Components

- **Kernels** (`kernels/`): Shared protocol and event schemas.
  - `kernel_copilot`: Primary kernel path (uses `copilot-cli`).
  - `kernel_host`: Runner for kernel containers.
- **Services** (`services/`):
  - `agent_host`: Rust service that manages sessions by spawning `kernel_host` containers; the Python service remains as transition reference code.
  - `client_service_rs`: Rust implementation of the public backend API. Clients should talk to this, not `agent_host` directly.
- **Clients** (`clients/`, `channels/`):
  - `webui`: TypeScript dashboard.
  - `cli_channel`: CLI session client for validating `client_service` contract.

## Development Workflow

### Commands

Use `just` for common tasks:
- `just bootstrap`: Install all dependencies (`uv sync` and `npm install`).
- `just check`: Run the full repo verification suite before finishing work.
- `just test`: Run all Python tests via `uv run pytest`.
- `just stack-up`: Start the full stack using Docker Compose.
- `just stack-down`: Stop the full stack.
- `just copilot-setup`: Run `kernels/kernel_host/spawn-kernel.sh setup` to authenticate Copilot.

Run `just check` after completing any code or documentation change. It covers
Python formatting, linting, type-checking, tests, web linting, npm tests when
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
- `client_service_rs`: `http://127.0.0.1:8002`
- `webui`: `http://127.0.0.1:8003`
