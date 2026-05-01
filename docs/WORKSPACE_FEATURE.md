# Agent Workspaces

Agent workspaces are named, persistent storage volumes that can be mounted into
agent kernel containers. A workspace is implemented as a Docker/Podman volume and
is exposed inside kernels at a stable path under `/workspace`.

This document describes the current implementation, the important design
decisions, and the code paths a new developer or agent should understand before
changing the feature.

## Product Model

A workspace is an AgentSpace-managed wrapper around a container volume.

Example flow:

1. A user creates a workspace named `TodoListCode` with ID `todo-list-code`.
2. A user creates or edits an agent and mounts that workspace as `rw`.
3. When a new session is started for that agent, `agent_host` creates the volume
   if needed and mounts it into the kernel container at
   `/workspace/todo-list-code`.
4. The agent can read and write files in that mounted path. The data persists
   across sessions and can be shared with other agents that mount the same
   workspace.

Workspaces can be mounted in two modes:

| Mode | Meaning |
|------|---------|
| `rw` | Read/write mount. The kernel can read and write the volume. |
| `ro` | Read-only mount. The kernel can read files but cannot modify them. |

Workspace configuration is applied when a kernel session is created. Existing
sessions keep the mounts they were started with; changing an agent's workspace
mounts only affects new or restarted sessions.

## Naming and Paths

Workspace IDs are user-facing identifiers. They must use lowercase letters,
digits, and single dashes. The same ID is used to derive the volume name and
container mount path.

For `workspace_id = "todo-list-code"`:

| Field | Value |
|-------|-------|
| Display name | Any user-facing string, for example `TodoListCode` |
| Docker/Podman volume | `agentspace-workspace-todo-list-code` |
| Kernel mount path | `/workspace/todo-list-code` |

Use the workspace ID in prompts and docs when telling agents where to read or
write files. Display names are only for the UI.

## High-Level Architecture

```
Web UI
  |
  | HTTP /api/workspaces, /api/agents, /api/sessions
  v
client_service_rs (:8002)
  - persists workspace records
  - persists per-agent workspace mount config
  - validates referenced workspaces when agents/sessions are created
  |
  | HTTP POST /sessions with workspace_mounts
  v
agent_host (:8001)
  - creates named Docker/Podman volumes lazily
  - mounts volumes into kernel containers
  - passes mount paths to kernels as additional accessible paths
  |
  v
kernel container
  - sees /workspace/<workspace_id>
```

The Rust `client_service_rs` implementation is the active client service. The
older Python `services/client_service` implementation is deprecated and should
not be treated as the source of truth for new work.

## Public API Contract

The Web UI talks to `client_service_rs` through the nginx `/api` proxy. The
service exposes workspace CRUD and accepts workspace mount configuration on
agents.

### Workspace Records

Workspace API responses have this shape:

```json
{
  "workspace_id": "todo-list-code",
  "name": "TodoListCode",
  "mount_path": "/workspace/todo-list-code",
  "volume_name": "agentspace-workspace-todo-list-code",
  "created_at": "2026-04-30T00:00:00Z",
  "updated_at": "2026-04-30T00:00:00Z"
}
```

Supported routes:

| Route | Purpose |
|-------|---------|
| `GET /workspaces` | List registered workspaces. |
| `POST /workspaces` | Create/register a workspace. |
| `GET /workspaces/{workspace_id}` | Fetch one workspace. |
| `PATCH /workspaces/{workspace_id}` | Update workspace metadata, currently the display name. |
| `DELETE /workspaces/{workspace_id}` | Unregister a workspace. Does not delete the underlying volume. |

Create payload:

```json
{
  "workspace_id": "todo-list-code",
  "name": "TodoListCode"
}
```

Update payload:

```json
{
  "name": "RenamedCode"
}
```

Deleting a workspace is rejected with `409 Conflict` if any agent still has that
workspace mounted. This prevents stale agent configs that reference missing
workspaces. Deletion only unregisters the workspace from AgentSpace; it does not
delete the Docker/Podman volume or its data.

### Agent Workspace Mounts

Agent records include `workspace_mounts`:

```json
{
  "agent_id": "todo-agent",
  "workspace_mounts": [
    {
      "workspace_id": "todo-list-code",
      "mode": "rw",
      "mount_path": "/workspace/todo-list-code"
    },
    {
      "workspace_id": "todo-list-items",
      "mode": "ro",
      "mount_path": "/workspace/todo-list-items"
    }
  ]
}
```

Create/update agent payloads accept the same mount list without `mount_path`:

```json
{
  "agent_id": "todo-agent",
  "name": "Todo Agent",
  "workspace_mounts": [
    { "workspace_id": "todo-list-code", "mode": "rw" },
    { "workspace_id": "todo-list-items", "mode": "ro" }
  ]
}
```

`client_service_rs` validates that:

- every referenced workspace exists;
- every referenced workspace ID has a valid format;
- a single agent cannot mount the same workspace more than once.

When a session is created, `client_service_rs` validates the agent's mounts again
before forwarding them to `agent_host`. This catches stale state if a workspace
record is removed or corrupted outside normal API flows.

## Persistence

The active persistence implementation is in `services/client_service_rs`.

Important Rust model types:

| Type | File | Purpose |
|------|------|---------|
| `WorkspaceRecord` | `services/client_service_rs/src/models.rs` | Registered workspace metadata. |
| `WorkspaceMountRecord` | `services/client_service_rs/src/models.rs` | Per-agent mount config. |
| `WorkspaceMountMode` | `services/client_service_rs/src/models.rs` | `rw` or `ro`. |

Store surfaces:

| Store | File | Notes |
|-------|------|-------|
| `WorkspaceStore` | `services/client_service_rs/src/store.rs` | Enum wrapper for in-memory and SQLite stores. |
| `InMemoryWorkspaceStore` | `services/client_service_rs/src/store.rs` | Used by tests and non-persistent app state. |
| `SqliteWorkspaceStore` | `services/client_service_rs/src/store/sqlite.rs` | Persistent workspace CRUD. |

SQLite schema additions:

- `workspaces` table:
  - `workspace_id TEXT PRIMARY KEY`
  - `name TEXT NOT NULL`
  - `created_at TEXT NOT NULL`
  - `updated_at TEXT NOT NULL`
- `agents.workspace_mounts_json TEXT NOT NULL DEFAULT '[]'`

The SQLite initializer includes a migration-style `ensure_column` call for
`agents.workspace_mounts_json`, so existing databases can be opened after the
feature is added.

## Runtime Mounting

Runtime mounting is handled by `services/agent_host`.

The client-service-to-agent-host boundary sends `workspace_mounts` in
`POST /sessions`:

```json
{
  "harness": "copilot-cli",
  "skills": [],
  "env": {},
  "workspace_mounts": [
    { "workspace_id": "todo-list-code", "mode": "rw" },
    { "workspace_id": "todo-list-items", "mode": "ro" }
  ]
}
```

`agent_host` maps each mount to:

- volume name: `agentspace-workspace-<workspace_id>`;
- container path: `/workspace/<workspace_id>`;
- read/write flag based on `mode`.

Volumes are created lazily if they do not already exist. This makes workspace
creation cheap: registering a workspace in `client_service_rs` does not need to
talk to Docker/Podman immediately.

The mount path is also added to the kernel's additional accessible paths. This
is important for CLI harnesses that need explicit directory allowlists, such as
Copilot, Codex, or Claude-style agents.

## Web UI

The Web UI feature is implemented in `clients/webui/src`.

Important files:

| File | Purpose |
|------|---------|
| `WorkspacesView.tsx` | Workspace management page. |
| `AgentsView.tsx` | Agent create/edit workspace mount controls and workspace mount summaries. |
| `api.ts` | HTTP client methods for workspaces and agent mount payloads. |
| `queries.ts` | React Query hooks for workspace data. |
| `types.ts` | TypeScript workspace and mount types. |
| `Sidebar.tsx` / `App.tsx` | Navigation entry and view routing. |

UI behavior:

- Workspaces are managed from the left-nav **Workspaces** page.
- Agent create/edit forms show workspace mount selectors when at least one
  workspace exists.
- Agent cards show the number of mounted workspaces and tags such as
  `todo-list-code:rw`.
- The edit form warns when changing an agent that already has active sessions,
  because those running kernels will not pick up new mounts until restarted.

## Validation and Testing

Use the standard repository check before finishing changes:

```sh
just check
```

For Rust-only iteration on `client_service_rs`:

```sh
just client-service-rs-check
```

That target runs:

- `cargo fmt --check --manifest-path services/client_service_rs/Cargo.toml`
- `cargo test --quiet --manifest-path services/client_service_rs/Cargo.toml`
- `cargo clippy --manifest-path services/client_service_rs/Cargo.toml --all-targets --all-features`

Workspace-specific Rust tests live in:

| Test file | Coverage |
|-----------|----------|
| `services/client_service_rs/tests/route_contract.rs` | Workspace CRUD and agent mount API shape. |
| `services/client_service_rs/tests/agent_host_proxy.rs` | Verifies `workspace_mounts` are forwarded to `agent_host`. |
| `services/client_service_rs/src/models.rs` tests | Workspace ID validation and summary shape. |
| `services/client_service_rs/src/store.rs` / `store/sqlite.rs` tests | Store behavior and SQLite persistence patterns. |

For end-to-end manual testing:

```sh
just stack-up
playwright-cli open http://127.0.0.1:8003 --headed
```

Exercise this flow:

1. Open **Workspaces** and create `todo-list-code` / `TodoListCode`.
2. Create `todo-list-items` / `TodoListItems`.
3. Open **Agents** and create or edit an agent with:
   - `todo-list-code` mounted `rw`;
   - `todo-list-items` mounted `ro`.
4. Start a new session from that agent.
5. Inspect the kernel container mounts:

```sh
podman ps --format '{{.ID}} {{.Names}} {{.Image}}'
podman inspect <kernel-container-id> --format '{{json .Mounts}}' \
  | jq '[.[] | select(.Destination|startswith("/workspace/")) | {Type,Name,Destination,RW}]'
```

Expected mount output includes:

```json
[
  {
    "Type": "volume",
    "Name": "agentspace-workspace-todo-list-code",
    "Destination": "/workspace/todo-list-code",
    "RW": true
  },
  {
    "Type": "volume",
    "Name": "agentspace-workspace-todo-list-items",
    "Destination": "/workspace/todo-list-items",
    "RW": false
  }
]
```

Shut the stack down after manual testing:

```sh
just stack-down
```

If bugs are found during manual testing, record repro steps in `BUGS.md`.

## Common Change Points

When adding or changing workspace behavior, check all of these layers:

1. **Rust models** — `services/client_service_rs/src/models.rs`
2. **Rust persistence** — `services/client_service_rs/src/store.rs` and
   `services/client_service_rs/src/store/sqlite.rs`
3. **Rust API** — `services/client_service_rs/src/api.rs`
4. **Agent-host client boundary** —
   `services/client_service_rs/src/agent_host.rs`
5. **Runtime mounting** —
   `services/agent_host/src/agent_host/service.py` and
   `services/agent_host/src/agent_host/app.py`
6. **Web UI** — `clients/webui/src/WorkspacesView.tsx`,
   `clients/webui/src/AgentsView.tsx`, `api.ts`, `queries.ts`, and `types.ts`
7. **Tests** — route contract, proxy, storage, and runtime mount tests

Avoid changing only one layer. For example, adding a new mount mode requires
model parsing, API validation, SQLite JSON compatibility, agent-host payload
handling, Docker mount options, Web UI selectors, and tests.

## Operational Notes

- `just stack-up` currently uses the Rust client service image through
  `compose.client-service-rs.env`.
- The Web UI is served at `http://127.0.0.1:8003`.
- The Rust client service is available at `http://127.0.0.1:8002`.
- `agent_host` is available at `http://127.0.0.1:8001`.
- Workspace volume data persists outside AgentSpace records. Deleting a
  workspace through the API unregisters it only; it intentionally does not delete
  Docker/Podman volume contents.
- Running sessions do not update their mounts after agent config changes.
  Restart the session to pick up new workspace configuration.
