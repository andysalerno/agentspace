# AgentSpace

Agents: see `AGENTS.md`.

The repo is currently centered on the kernel milestone:

- `kernel`: shared protocol and JSONL event schema
- `kernel_echo`: reference in-process kernel
- `kernel_copilot`: `copilot-cli` kernel adapter
- `kernel_host`: runner plus one-session HTTP service mode for kernel containers
- `agent_host`: session manager that spawns and supervises `kernel_host` containers
- `client_service`: client-facing API over `agent_host`
- `webui`: TypeScript dashboard over `client_service`
- `cli_channel`: proof-of-concept channel client over `client_service`

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

The current `agent_host` slice is a containerized FastAPI service that manages sessions by spawning one `kernel_host` container per session.

Start it with:

```powershell
.\services\agent_host\run-service.ps1 start
```

Stop it with:

```powershell
.\services\agent_host\run-service.ps1 stop
```

Default endpoint: `http://127.0.0.1:8001`

Architecture note:

- `agent_host` does not mount Copilot login state directly
- spawned `kernel_host` containers mount the shared Copilot config volume instead
- `agent_host` talks to those kernel containers over an internal HTTP API

Currently implemented endpoints:

- `GET /healthz`
- `POST /sessions`
- `GET /sessions`
- `GET /sessions/{session_id}`
- `POST /sessions/{session_id}/messages`
- `GET /sessions/{session_id}/history`
- `POST /sessions/{session_id}/reset`
- `DELETE /sessions/{session_id}`

## Client Service

`client_service` is now the intended public backend API. Clients should talk to it, not to `agent_host` directly.

Start it with:

```powershell
.\services\client_service\run-service.ps1 start
```

Stop it with:

```powershell
.\services\client_service\run-service.ps1 stop
```

Default endpoint: `http://127.0.0.1:8002`

Current endpoints:

- `GET /healthz`
- `POST /agents`
- `GET /agents`
- `GET /agents/{agent_id}`
- `PATCH /agents/{agent_id}`
- `DELETE /agents/{agent_id}`
- `POST /sessions`
- `GET /sessions`
- `GET /sessions/{session_id}`
- `GET /sessions/{session_id}/messages`
- `POST /sessions/{session_id}/messages`
- `POST /sessions/{session_id}/reset`
- `DELETE /sessions/{session_id}`
- `POST /channels`
- `GET /channels`
- `GET /channels/{channel_id}`
- `GET /channels/{channel_id}/messages`
- `POST /channels/{channel_id}/messages`
- `POST /channels/{channel_id}/reset`
- `DELETE /channels/{channel_id}`
- `GET /kernels`

Channel notes:

- a channel registers itself with `client_service`
- `client_service` creates a backing long-lived client-facing session immediately
- repeated channel messages are routed to that same session
- channel reset keeps the same client-facing session id but swaps to a fresh underlying `agent_host` session

## Web UI

`webui` is a deliberately simple TypeScript dashboard over `client_service`.

Start it with:

```powershell
.\clients\webui\run-service.ps1 start
```

Stop it with:

```powershell
.\clients\webui\run-service.ps1 stop
```

Default endpoint: `http://127.0.0.1:8003`

It currently supports:

- viewing, creating, and deleting agents
- starting sessions and chatting with them
- viewing existing sessions, including channel-backed sessions
- viewing registered channels and their mapped sessions
- viewing active kernel sessions exposed through `client_service`

## CliChannel

`cli_channel` is the first proof-of-concept channel client. It is separate from the future native CLI client and exists only to validate the channel protocol.

Run it against a created agent:

```powershell
uv run --package cli-channel -m cli_channel --agent-id <agent_id> --name terminal-1
```

Supported commands:

- normal input sends a channel message
- `/reset` resets the backing session while preserving the channel id
- `/exit` exits the client
