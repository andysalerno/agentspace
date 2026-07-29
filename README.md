# AgentSpace

Agents: see `AGENTS.md`.

The repo is currently centered on the kernel milestone:

- `kernel`: shared protocol and JSONL event schema
- `kernel_echo`: reference in-process kernel
- `kernel_copilot`: `copilot-cli` kernel adapter
- `kernel_host`: runner plus one-session HTTP service mode for kernel containers
- `agent_host`: session manager that spawns and supervises `kernel_host` containers
- `client_service` (`services/client_service_rs`): client-facing API over `agent_host`
- `memory` (`services/memory_rs`): local CLI and private HTTP memory service
- `git_agent`: internal GitAgent service for repo access and patch submission
- `webui`: TypeScript dashboard over `client_service`
- `cli_channel`: proof-of-concept CLI session client over `client_service`

For now, keep `copilot-cli` as the only real kernel path.

## Validate

```powershell
$env:UV_CACHE_DIR='C:\Users\andys\AppData\Local\Temp\uv-cache'
uv run pytest
uv run ruff check .
uv run pyright
```

## Development Container

The openSUSE development image includes Podman and Podman Compose. Start the
host's rootless Podman API socket before creating the container:

```sh
systemctl --user enable --now podman.socket
just dev-start
podman logs --follow agentspace-dev
```

`just dev-start` discovers the host socket and mounts it at
`/run/podman/podman.sock`. Set `PODMAN_SOCKET` when the socket uses a
nonstandard path.

The container runs a VS Code tunnel named `agentspace-dev`. On the first start,
follow the container logs to complete the device login flow. The CLI
authentication metadata, VS Code server, and extensions are stored under
`/home/dev`, which uses a persistent named volume by default, so later container
starts reuse the login. Pressing `Ctrl+C` stops following the logs without
stopping the container.

Open a shell in the running development container with:

```sh
just dev-shell
```

The mounted socket connects the development container to the host Podman
daemon, so `podman images` and `podman ps` show the host user's containers and
images. An isolated store would require running a separate nested Podman daemon
instead of using the host socket.

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

The Dockerized `agent_host` service uses `services/agent_host_rs/Dockerfile` with the repository root as its build context and manages sessions by spawning one `kernel_host` container per session.

Start it with:

```powershell
.\services\agent_host_rs\run-service.ps1 start
```

Stop it with:

```powershell
.\services\agent_host_rs\run-service.ps1 stop
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

## Agent Memory

The built-in `memory` skill provides opt-in, durable memory to hosted agents.
Enable **memory** on an agent in the Agents page, then start a new session or
reset an existing one. The kernel image includes the `memory` CLI; enabled
sessions receive its instructions and share an installation-scoped persistent
volume.

The memory corpus is shared by every agent that enables the skill. Never store
credentials, tokens, secrets, or sensitive personal information in it. Agents
without the skill do not receive the memory volume or memory instructions.

The volume name defaults to `agentspace-memory-data` and can be overridden with
`AGENTSPACE_MEMORY_VOLUME`. It survives normal session deletion, reset,
`just stack-down`, and `just stack-up`. Removing that named volume permanently
deletes the corpus.

The default stack also runs a private `memory` HTTP service on the internal
network. It uses the same `memory` binary and named volume as local
memory-enabled kernels. Clients must use the `client_service` proxy under
`/memory`; the memory service does not publish a host port.

The CLI supports three deployment modes:

```sh
# Explicit local store
memory --root /path/to/memory pages ls

# In-stack or otherwise reachable service
AGENTSPACE_MEMORY_URI=http://memory:8005 memory pages ls

# Externally hosted service
memory --uri https://memory.example.internal pages ls
```

`--uri` or `AGENTSPACE_MEMORY_URI` selects HTTP mode and never falls back to
local storage after an error. `--root` or `AGENTSPACE_MEMORY_DIR` selects local
mode. Run a standalone service with
`memory --serve --root /path/to/memory --host 127.0.0.1 --port 8005`.

Configure the public proxy with `CLIENT_SERVICE_MEMORY_BASE_URL` and its
bounded upstream timeout with `CLIENT_SERVICE_MEMORY_TIMEOUT`. It exposes the
memory service routes at `/memory/healthz` and `/memory/v1/...`.

The Web UI's **Memory** page uses that proxy to browse the page tree, search,
filter by tags, edit and preview Markdown, inspect links and backlinks, move or
delete pages, and view integrity findings. Browser writes always include the
revision that was loaded. If an agent or another browser changes the page
first, the stale edit is retained as a draft but cannot overwrite the newer
content; reload the latest revision before saving again. The page explicitly
distinguishes a healthy empty corpus from an unavailable memory service.

### Back up and restore memory

Stop the stack before taking a consistent filesystem backup:

```sh
just stack-down
podman run --rm \
  -v agentspace-memory-data:/source:ro \
  -v "$PWD":/backup \
  docker.io/library/alpine \
  tar czf /backup/agentspace-memory-backup.tgz -C /source .
```

To restore into an empty volume:

```sh
just stack-down
podman volume create agentspace-memory-data
podman run --rm \
  -v agentspace-memory-data:/target \
  -v "$PWD":/backup:ro \
  docker.io/library/alpine \
  tar xzf /backup/agentspace-memory-backup.tgz -C /target
just stack-up
```

Replace `agentspace-memory-data` in both commands when
`AGENTSPACE_MEMORY_VOLUME` selects another volume. Backups contain the shared
Markdown corpus and should be protected accordingly.

To intentionally erase all memory, stop the stack and remove the configured
named volume:

```sh
just stack-down
podman volume rm agentspace-memory-data
```

Disabling the skill, deleting agents or sessions, and normal stack recreation
never remove this volume.

## Client Service

`client_service` is the intended public backend API. Clients should talk to it, not to `agent_host` directly. The implementation lives in `services/client_service_rs`.

Start it with:

```sh
./services/client_service_rs/run-service.sh
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
- `GET /kernels`
- `/memory/healthz` and `/memory/v1/...` proxy routes

Session metadata notes:

- clients can set optional `channel_name` and `client_type` when creating a session
- persistence is keyed only by `session_id`
- external adapters are responsible for remembering that `session_id`

## GitAgent

`git_agent` is the internal service that owns the shared Git repository and
patch submission workflow. In compose it is reachable to kernels as
`http://gitagent:8004`.

Default endpoint: `http://127.0.0.1:8004`
Default in-network git remote: `http://gitagent:8004/repo.git`

GitAgent stores its repository and request database in the stable named Docker
volume `${GITAGENT_DATA_VOLUME:-agentspace-git-agent-data}`. This volume
persists across `just stack-down` and `just stack-up`; remove it only when you
intentionally want to erase GitAgent state.

On first run, agents may not be able to clone until the first patch has been
accepted. Use the `gitagent-helper` skill's `clone` command; it falls back to an
empty local repo with `origin` set to GitAgent, so agents can create a new
project, commit locally, and submit the initial patch with the all-zero base SHA.

The same volume is exposed through `client_service` as the built-in `git-agent`
workspace. It always appears on the Workspaces page and can be opened in VS Code
like a normal workspace, but it cannot be edited, cloned, deleted, or replaced by
a user-created workspace.

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
- viewing existing sessions, including sessions created by other clients
- viewing the session source metadata attached at creation time
- viewing active kernel sessions exposed through `client_service`
- browsing and revision-safe editing of shared agent memory

Kernel VS Code links are opened against the web UI's current browser host when
the kernel reports a loopback or wildcard host. For remote deployments, make sure
`AGENT_HOST_KERNEL_VSCODE_HOST_IP` is `0.0.0.0` or another externally reachable
interface, and allow the dynamically published kernel VS Code ports through the
host firewall.

Spawned kernel containers also publish container port `8081` to a dynamic host
port by default. The host URL is exposed as `free_port_url`, and the container
receives `KERNEL_FREE_PORT=8081` so agents can bind ad hoc services there.

## CliChannel

`cli_channel` is the first proof-of-concept CLI session client. It is separate from the future native CLI client and exists only to validate the client-service session contract.

Start a new session:

```powershell
uv run --package cli-channel -m cli_channel --agent-id <agent_id> --name terminal-1
```

Resume an existing session:

```powershell
uv run --package cli-channel -m cli_channel --session-id <session_id>
```

Supported commands:

- normal input sends a session message
- `/reset` resets the backing kernel session while preserving the client-facing `session_id`
- `/exit` exits the client
