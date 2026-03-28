# AgentSpace

Agents: see `AGENTS.md`.

The repo is currently centered on the kernel milestone:

- `kernel`: shared protocol and JSONL event schema
- `kernel_echo`: reference in-process kernel
- `kernel_copilot`: `copilot-cli` kernel adapter
- `kernel_host`: runner, Docker image, and launch scripts
- `agent_host`: in-memory session manager with a FastAPI service wrapper

For now, keep `copilot-cli` as the only real kernel path.

## Validate

```powershell
$env:UV_CACHE_DIR='C:\Users\andys\AppData\Local\Temp\uv-cache'
uv run pytest
uv run ruff check .
uv run pyright
```

## Dockerized Copilot Flow

Authenticate Copilot once inside the container environment:

```powershell
.\kernels\kernel_host\spawn-kernel.ps1 setup
```

Then run a prompt through the kernel host:

```powershell
.\kernels\kernel_host\spawn-kernel.ps1 "Summarize this repository"
```

The launcher now:

- brings down previous compose resources before every run
- persists Copilot config and session state in the `copilot-config` volume

Before using the Docker flow, set `KERNEL_WORKDIR` in [kernels/kernel_host/.env.example](/C:/repos/agentspace/kernels/kernel_host/.env.example) or your local `.env`. It is intentionally not defaulted by compose.

To resume a previous Copilot session, set `COPILOT_SESSION_ID` in [kernels/kernel_host/.env.example](/C:/repos/agentspace/kernels/kernel_host/.env.example).

## Agent Host

The first `agent_host` slice now exists as a containerized FastAPI service that manages kernel-backed sessions in memory.

Start it with:

```powershell
.\services\agent_host\run-service.ps1 start
```

Stop it with:

```powershell
.\services\agent_host\run-service.ps1 stop
```

Default endpoint: `http://127.0.0.1:8001`

Currently implemented endpoints:

- `GET /healthz`
- `POST /sessions`
- `GET /sessions`
- `GET /sessions/{session_id}`
- `POST /sessions/{session_id}/messages`
- `GET /sessions/{session_id}/history`
- `POST /sessions/{session_id}/reset`
- `DELETE /sessions/{session_id}`
