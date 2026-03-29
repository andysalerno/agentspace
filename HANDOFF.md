# Handoff

## Current State

The repo is no longer just a kernel prototype. There is now a working vertical slice for:

- `kernel`
  Shared protocol and event model.
- `kernel_copilot`
  Wraps `copilot-cli` and maps JSONL output into standard kernel events.
- `kernel_host`
  Can run as:
  - one-shot runner: `python -m kernel_host.runner`
  - one-session HTTP service inside a container: `python -m kernel_host.api_main`
- `agent_host`
  FastAPI service that manages sessions by spawning one `kernel_host` container per session through Docker.

This means the intended layering is now mostly in place:

- `agent_host` does not run Copilot directly
- `kernel_host` containers own harness lifecycle and login/session persistence
- `agent_host` talks to those kernel containers over HTTP

## Important Architectural Boundary

This was the main correction made in the latest pass.

Earlier prototype:

- `agent_host` imported `CopilotKernel` directly
- `agent_host` mounted the Copilot config volume

Current model:

- `agent_host` mounts the Docker socket
- `agent_host` spawns `kernel_host` containers on the Docker network
- spawned `kernel_host` containers mount the shared Copilot config volume
- only `kernel_host` containers need access to Copilot auth state

## Recent Commits

Most relevant recent checkpoints:

- `d3fa98a` `feat(kernel): add session resumption, runtime config, and tool result mapping`
- `7dfe5f2` `fix(kernel): rebuild docker runner and create workdirs`
- `0e7a6f1` `feat(agent_host): add copilot-backed session service`
- `e2e9103` `test(agent_host): cover HTTP session lifecycle`
- `77a7957` `refactor(agent_host): spawn kernel containers via docker`

## Key Files

### Kernel layer

- [kernels/kernel/src/kernel/protocol.py](/C:/repos/agentspace/kernels/kernel/src/kernel/protocol.py)
  `KernelConfig`, `Kernel` protocol, `resume_token`
- [kernels/kernel_copilot/src/kernel_copilot/__init__.py](/C:/repos/agentspace/kernels/kernel_copilot/src/kernel_copilot/__init__.py)
  Copilot wrapper, event mapping, resume token capture
- [kernels/kernel_host/src/kernel_host/runner.py](/C:/repos/agentspace/kernels/kernel_host/src/kernel_host/runner.py)
  one-shot mode
- [kernels/kernel_host/src/kernel_host/service.py](/C:/repos/agentspace/kernels/kernel_host/src/kernel_host/service.py)
  per-session service mode
- [kernels/kernel_host/src/kernel_host/app.py](/C:/repos/agentspace/kernels/kernel_host/src/kernel_host/app.py)
  FastAPI for kernel container
- [kernels/kernel_host/src/kernel_host/api_main.py](/C:/repos/agentspace/kernels/kernel_host/src/kernel_host/api_main.py)
  uvicorn entrypoint for kernel container API

### Agent host layer

- [services/agent_host/src/agent_host/service.py](/C:/repos/agentspace/services/agent_host/src/agent_host/service.py)
  Docker-backed session manager
- [services/agent_host/src/agent_host/app.py](/C:/repos/agentspace/services/agent_host/src/agent_host/app.py)
  FastAPI API surface
- [services/agent_host/src/agent_host/__main__.py](/C:/repos/agentspace/services/agent_host/src/agent_host/__main__.py)
  uvicorn entrypoint

### Docker / ops

- [kernels/kernel_host/Dockerfile](/C:/repos/agentspace/kernels/kernel_host/Dockerfile)
- [kernels/kernel_host/compose.copilot.yaml](/C:/repos/agentspace/kernels/kernel_host/compose.copilot.yaml)
- [kernels/kernel_host/spawn-kernel.sh](/C:/repos/agentspace/kernels/kernel_host/spawn-kernel.sh)
- [kernels/kernel_host/spawn-kernel.ps1](/C:/repos/agentspace/kernels/kernel_host/spawn-kernel.ps1)
- [services/agent_host/Dockerfile](/C:/repos/agentspace/services/agent_host/Dockerfile)
- [services/agent_host/compose.yaml](/C:/repos/agentspace/services/agent_host/compose.yaml)
- [services/agent_host/run-service.sh](/C:/repos/agentspace/services/agent_host/run-service.sh)
- [services/agent_host/run-service.ps1](/C:/repos/agentspace/services/agent_host/run-service.ps1)

### Docs

- [README.md](/C:/repos/agentspace/README.md)
- [docs/OVERVIEW.md](/C:/repos/agentspace/docs/OVERVIEW.md)
- [PLAN.md](/C:/repos/agentspace/PLAN.md)
- [PLAN_KERNEL.md](/C:/repos/agentspace/PLAN_KERNEL.md)

## How To Run

### 1. Kernel host one-shot

Authenticate Copilot if needed:

```powershell
.\kernels\kernel_host\spawn-kernel.ps1 setup
```

Run a prompt:

```powershell
.\kernels\kernel_host\spawn-kernel.ps1 "Reply with exactly: test-ok"
```

Notes:

- launcher rebuilds before running
- launcher tears down old compose resources before and after run
- shared Copilot volume name is `agentspace-kernel_copilot-config`

### 2. Agent host service

Start:

```powershell
.\services\agent_host\run-service.ps1 start
```

Stop:

```powershell
.\services\agent_host\run-service.ps1 stop
```

Default URL:

```text
http://127.0.0.1:8001
```

Example:

```bash
curl -X POST http://127.0.0.1:8001/sessions \
  -H 'Content-Type: application/json' \
  -d '{"harness":"copilot-cli","cwd":"/tmp/agent-session"}'
```

Then:

```bash
curl -X POST http://127.0.0.1:8001/sessions/<session_id>/messages \
  -H 'Content-Type: application/json' \
  -d '{"message":"Reply with exactly: hello-from-agent-host"}'
```

## Environment / Runtime Details

### Shared Copilot volume

The shared Copilot auth volume is:

```text
agentspace-kernel_copilot-config
```

This is intentionally mounted only into spawned kernel containers.

### Agent host container runtime assumptions

`agent_host` compose mounts:

- `/var/run/docker.sock:/var/run/docker.sock`

and sets:

- `AGENT_HOST_KERNEL_IMAGE=agentspace-kernel-kernel:latest`
- `AGENT_HOST_DOCKER_NETWORK=agentspace-agent-host_default`
- `AGENT_HOST_KERNEL_BASE_URL_TEMPLATE=http://{container_name}:8000`
- `AGENT_HOST_COPILOT_VOLUME=agentspace-kernel_copilot-config`

### Workdir handling

`KERNEL_WORKDIR` is intentionally not defaulted inside kernel compose. Users are expected to choose it.

For `agent_host`, [services/agent_host/.env.example](/C:/repos/agentspace/services/agent_host/.env.example) documents the default workdir passed to spawned kernel containers.

## Validated Behavior

### Automated

Latest full validation passed:

- `uv run python -m pytest`
- `uv run ruff check .`
- `uv run pyright`

Important note:

- use `uv run python -m pytest`, not `uv run pytest`
- on this Windows/uv setup, `uv run pytest` can lose the workspace packages on import

### Manual Docker e2e

Confirmed working:

1. build `kernel_host` image
2. start `agent_host`
3. `POST /sessions`
4. `POST /sessions/{id}/messages`
5. `agent_host` spawns a separate `kernel_host` container
6. that kernel container runs real `copilot-cli`
7. second turn resumes context successfully

Specific validation that succeeded:

- first prompt: remember token `mango-signal`
- second prompt: ask for remembered token
- response returned `mango-signal`

That is the strongest proof that session state is preserved through the spawned kernel container and not held in `agent_host`.

## Current API Surfaces

### agent_host

- `GET /healthz`
- `POST /sessions`
- `GET /sessions`
- `GET /sessions/{session_id}`
- `POST /sessions/{session_id}/messages`
- `GET /sessions/{session_id}/history`
- `POST /sessions/{session_id}/reset`
- `DELETE /sessions/{session_id}`

### kernel_host service mode

- `GET /healthz`
- `GET /session`
- `POST /messages`
- `GET /history`
- `POST /reset`
- `DELETE /session`

## Known Limitations

These are the most relevant next issues.

### 1. `agent_host` persistence is still in-memory

Session records are lost when `agent_host` restarts.

Needed later:

- persistence library / store
- restoring or reconciling kernel containers on startup

### 2. `agent_host` cleanup is basic

It removes spawned containers on explicit destroy/reset. There is no automatic reconciliation for orphaned kernel containers on `agent_host` crash or restart.

Likely next step:

- add startup reconciliation by label or naming prefix
- label spawned containers with session metadata

### 3. kernel container API is simple request/response, not streaming

Right now `agent_host` waits for `/messages` to complete and then returns a buffered event list.

Needed later:

- streaming attach/output path
- observer/fan-out support
- maybe SSE or websocket transport

### 4. `kernel_host` service creates a fresh kernel object per turn

This is intentional for now because Copilot session resumption is carried by resume token plus persisted harness state, but it means:

- no in-memory kernel continuity is assumed
- this matches the architecture reasonably well, but should be kept in mind if a future harness needs a long-lived process

### 5. no client-service / web / CLI yet

Still missing:

- `client-service`
- `client-web`
- `client-cli`
- `store`
- `channels`

## Suggested Next Steps

If picking this up tomorrow, strongest next tasks are:

1. Add container labels and reconciliation in `agent_host`
   So sessions can be discovered/cleaned up after crash or restart.

2. Add true streaming from kernel containers
   Probably expose a streaming endpoint from `kernel_host`, then proxy it from `agent_host`.

3. Introduce persistence
   Start with SQLite-backed session metadata.

4. Add a minimal client-service or CLI
   Even a thin CLI over `agent_host` would make testing much faster.

5. Tighten Docker runtime config
   The current Docker socket mount is acceptable for a prototype but should eventually be isolated/hardened.

## Operational Notes

- At the end of the last work session, no containers were left running.
- The repo was clean after commit.
- The most recent major refactor commit is:

```text
77a7957 refactor(agent_host): spawn kernel containers via docker
```

If resuming, start by reading:

1. [HANDOFF.md](/C:/repos/agentspace/HANDOFF.md)
2. [README.md](/C:/repos/agentspace/README.md)
3. [services/agent_host/src/agent_host/service.py](/C:/repos/agentspace/services/agent_host/src/agent_host/service.py)
4. [kernels/kernel_host/src/kernel_host/service.py](/C:/repos/agentspace/kernels/kernel_host/src/kernel_host/service.py)
