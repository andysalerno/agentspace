# Agent Workspaces

Agent workspaces are named, persistent storage volumes that can be mounted into
agent kernel containers. A workspace is implemented as a Docker/Podman volume and
is exposed to agents at a stable path under `/workspace`.

This document describes the current implementation, the important design
decisions, and the code paths a new developer or agent should understand before
changing the feature.

## Product Model

A workspace is an AgentSpace-managed wrapper around a container volume.

Example flow:

1. A user creates a workspace named `TodoListCode` with ID `todo-list-code`.
2. A user creates or edits an agent and mounts that workspace as `rw`.
3. When a new session is started for that agent, `agent_host` creates a
   per-session scratch volume at `/workspace` and mounts persistent workspace
   volumes directly under `/workspace/<workspace_id>`, such as
   `/workspace/todo-list-code`.
4. The agent sees mounted workspaces in its cwd (`/workspace`) while normal
   scratch files also live in `/workspace`.
5. When the session is deleted or the kernel is killed from the UI, the user is
   prompted to save `/workspace` as a new workspace or destroy the scratch
   volume forever. Existing mounted workspaces are not copied into the saved
   scratch snapshot.
6. A registered workspace can be cloned into a new workspace, or opened in a
   standalone code-server container from the Workspaces page.

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
public in-container path.

For `workspace_id = "todo-list-code"`:

| Field | Value |
|-------|-------|
| Display name | Any user-facing string, for example `TodoListCode` |
| Docker/Podman volume | `agentspace-workspace-todo-list-code` |
| Kernel mount path | `/workspace/todo-list-code` |

Use the public `/workspace/<workspace_id>` path in prompts and docs when telling
agents where to read or write files. Display names are only for the UI. The
session save flow explicitly excludes mounted workspace IDs so saving the parent
`/workspace` scratch volume does not include existing mounted workspaces.

## High-Level Architecture

```
Web UI
  |
  | HTTP /api/workspaces, /api/agents, /api/sessions
  v
client_service (:8002)
  - persists workspace records
  - persists per-agent workspace mount config
  - validates referenced workspaces when agents/sessions are created
  |
  | HTTP POST /sessions with workspace_mounts
  v
agent_host (:8001)
  - creates named Docker/Podman volumes lazily
  - mounts one scratch volume at /workspace per session
  - mounts persistent workspaces directly under /workspace/<workspace_id>
  - snapshots /workspace into new workspace volumes on save
  |
  v
kernel container
  - starts in /workspace
  - sees mounted workspaces at /workspace/<workspace_id>
```

`client_service` is the public API for workspace CRUD and workspace-aware
session creation.

## Public API Contract

The Web UI talks to `client_service` through the nginx `/api` proxy. The
service exposes workspace CRUD and accepts workspace mount configuration on
agents.

### Workspace Records

Workspace API responses have this shape:

```json
{
  "workspace_id": "todo-list-code",
  "name": "TodoListCode",
  "status": "ready",
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
| `POST /workspaces/{workspace_id}/clone` | Copy a ready workspace volume into a new registered workspace. |
| `POST /workspaces/{workspace_id}/vscode` | Start or reuse a workspace editor container and return its VS Code URL. |
| `POST /sessions/{session_id}/workspace/save` | Snapshot a session scratch workspace into a new registered workspace. |

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

Workspace `status` values are:

| Status | Meaning |
|--------|---------|
| `creating` | A save operation has registered the workspace and is snapshotting data into the volume. |
| `ready` | The workspace can be mounted by agents. Manually created workspaces start in this state. |
| `failed` | A save operation failed after registration; the workspace is visible for diagnosis but cannot be mounted. |

Agent mount validation rejects workspaces that are not `ready`.

Save-session-workspace payload:

```json
{
  "workspace_id": "saved-session-workspace",
  "name": "Saved Session Workspace"
}
```

`client_service` implements the robust save flow:

1. Insert the workspace record as `creating`.
2. Ask `agent_host` to snapshot the session scratch volume into
   `agentspace-workspace-<workspace_id>`.
3. Mark the workspace `ready` if the snapshot succeeds.
4. Mark it `failed` and return the upstream error if snapshotting fails.

Clone payload:

```json
{
  "workspace_id": "cloned-workspace",
  "name": "Cloned Workspace"
}
```

The clone flow uses the same status lifecycle as saving a session workspace:
`client_service` inserts the target as `creating`, asks `agent_host` to copy
the source volume to `agentspace-workspace-<workspace_id>`, and then marks the
target `ready` or `failed`. The source workspace must already be `ready`.

Open-in-VS-Code response:

```json
{
  "workspace_id": "todo-list-code",
  "volume_name": "agentspace-workspace-todo-list-code",
  "container_name": "agentspace-workspace-editor-todo-list-code",
  "vscode_url": "http://127.0.0.1:45678"
}
```

The Web UI should pass `vscode_url` through `browserReachableLocalUrl()` before
opening it so loopback/wildcard host URLs work from the browser.

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

`client_service` validates that:

- every referenced workspace exists;
- every referenced workspace is `ready`;
- every referenced workspace ID has a valid format;
- a single agent cannot mount the same workspace more than once.

When a session is created, `client_service` validates the agent's mounts again
before forwarding them to `agent_host`. This catches stale state if a workspace
record is removed or corrupted outside normal API flows.

## Persistence

Workspace persistence lives in `services/client_service_rs`.

Important model types:

| Type | File | Purpose |
|------|------|---------|
| `WorkspaceRecord` | `services/client_service_rs/src/models.rs` | Registered workspace metadata. |
| `WorkspaceStatus` | `services/client_service_rs/src/models.rs` | `creating`, `ready`, or `failed`. |
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
  - `status TEXT NOT NULL DEFAULT 'ready'`
  - `created_at TEXT NOT NULL`
  - `updated_at TEXT NOT NULL`
- `agents.workspace_mounts_json TEXT NOT NULL DEFAULT '[]'`

The SQLite initializer includes migration-style `ensure_column` calls for
`agents.workspace_mounts_json` and `workspaces.status`, so existing databases can
be opened after the feature is added.

## Runtime Mounting

Runtime mounting is handled by `services/agent_host_rs`.

The client-service-to-agent-host boundary sends `workspace_mounts` in
`POST /sessions`:

```json
{
  "harness": "acp",
  "skills": [],
  "env": {},
  "workspace_mounts": [
    { "workspace_id": "todo-list-code", "mode": "rw" },
    { "workspace_id": "todo-list-items", "mode": "ro" }
  ]
}
```

`agent_host` maps each persistent mount to:

- volume name: `agentspace-workspace-<workspace_id>`;
- container path: `/workspace/<workspace_id>`;
- read/write flag based on `mode`.

`agent_host` also creates a per-session scratch volume named
`agentspace-session-workspace-<session_id_prefix>` and mounts it at `/workspace`
for every kernel, even when the agent has no persistent workspaces enabled. This
volume is deleted when the runtime session is destroyed unless the user saves it
first.

Persistent workspace volumes are created lazily if they do not already exist.
This makes workspace creation cheap: registering a workspace in
`client_service` does not need to talk to Docker/Podman immediately.

The mount path is also added to the kernel's additional accessible paths. This
is important for CLI harnesses that need explicit directory allowlists, such as
Copilot, Codex, or Claude-style agents.

### Saving Session Scratch Workspaces

`client_service` calls `agent_host` through:

```http
POST /sessions/{agent_host_session_id}/workspace/snapshot
```

Payload:

```json
{
  "workspace_id": "saved-session-workspace",
  "volume_name": "agentspace-workspace-saved-session-workspace",
  "exclude_names": ["todo-list-code", ".agents"]
}
```

The snapshot helper copies top-level entries from `/workspace` into the target
workspace volume. `exclude_names` prevents copying mounted persistent workspaces
such as `/workspace/todo-list-code` and the ACP skills mount under
`/workspace/.agents/skills`.

### Cloning and Opening Workspaces

`client_service` calls `agent_host` through:

```http
POST /workspaces/clone
POST /workspaces/vscode
```

`/workspaces/clone` copies from an existing source volume to a new target
workspace volume with the same copy helper used for session snapshots. The
source volume is opened directly; it is not created if missing, so a stale
`ready` record with no backing volume fails instead of producing an empty clone.

`/workspaces/vscode` starts a long-running workspace editor container named
`agentspace-workspace-editor-<workspace_id>` if one is not already running. It
uses the kernel image so the existing code-server installation can be reused,
mounts the workspace volume at `/workspace`, publishes the configured VS Code
port, and returns the same URL shape used by kernel VS Code sessions. The
workspace volume must already exist; the endpoint does not create missing
volumes.

## Web UI

The Web UI feature is implemented in `clients/webui/src`.

Important files:

| File | Purpose |
|------|---------|
| `WorkspacesView.tsx` | Workspace management page. |
| `AgentsView.tsx` | Agent create/edit workspace mount controls and workspace mount summaries. |
| `ChatView.tsx` / `SessionsView.tsx` / `KernelsView.tsx` | Save-or-destroy prompts before session/kernel deletion. |
| `saveWorkspacePrompt.ts` | Shared browser prompt flow for naming saved scratch workspaces. |
| `api.ts` | HTTP client methods for workspaces and agent mount payloads. |
| `queries.ts` | React Query hooks for workspace data. |
| `types.ts` | TypeScript workspace and mount types. |
| `Sidebar.tsx` / `App.tsx` | Navigation entry and view routing. |

UI behavior:

- Workspaces are managed from the left-nav **Workspaces** page.
- Ready workspaces can be cloned from the **Clone** button.
- Ready workspaces can be opened from the **Open in VS Code** button, which
  starts/reuses the workspace editor container and opens the returned URL in a
  new tab.
- Agent create/edit forms show workspace mount selectors when at least one
  workspace exists.
- Agent cards show the number of mounted workspaces and tags such as
  `todo-list-code:rw`.
- The edit form warns when changing an agent that already has active sessions,
  because those running kernels will not pick up new mounts until restarted.
- Deleting a session or killing a linked kernel prompts the user to save the
  session scratch `/workspace` as a new workspace or destroy it forever. Orphan
  kernels without a linked client session can only be killed/destroyed.

## Validation and Testing

Use the standard repository check before finishing changes:

```sh
just check
```

For client-service iteration:

```sh
just client-service-check
```

That target runs:

- `cargo fmt --check --manifest-path services/client_service_rs/Cargo.toml`
- `cargo test --quiet --manifest-path services/client_service_rs/Cargo.toml`
- `cargo clippy --manifest-path services/client_service_rs/Cargo.toml --all-targets --all-features`

Workspace-specific tests live in:

| Test file | Coverage |
|-----------|----------|
| `services/client_service_rs/tests/route_contract.rs` | Workspace CRUD, clone, VS Code, and agent mount API shape. |
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
  | jq '[.[] | select(.Destination|startswith("/workspace")) | {Type,Name,Destination,RW}]'
```

Expected mount output includes:

```json
[
  {
    "Type": "volume",
    "Name": "agentspace-session-workspace-<session-prefix>",
    "Destination": "/workspace",
    "RW": true
  },
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

Inside the container, `findmnt -R /workspace` should show enabled workspaces
mounted directly under `/workspace/<workspace_id>`.

Shut the stack down after manual testing:

```sh
just stack-down
```

If bugs are found during manual testing, record repro steps in `BUGS.md`.

## Common Change Points

When adding or changing workspace behavior, check all of these layers:

1. **Models** — `services/client_service_rs/src/models.rs`
2. **Persistence** — `services/client_service_rs/src/store.rs` and
   `services/client_service_rs/src/store/sqlite.rs`
3. **API** — `services/client_service_rs/src/api.rs`
4. **Agent-host client boundary** —
   `services/client_service_rs/src/agent_host.rs`
5. **Runtime mounting** —
   `services/agent_host_rs/src/docker_runtime.rs` and
   `services/agent_host_rs/src/sessions.rs`
6. **Web UI** — `clients/webui/src/WorkspacesView.tsx`,
   `clients/webui/src/AgentsView.tsx`, `api.ts`, `queries.ts`, and `types.ts`
7. **Tests** — route contract, proxy, storage, and runtime mount tests

Avoid changing only one layer. For example, adding a new mount mode requires
model parsing, API validation, SQLite JSON compatibility, agent-host payload
handling, Docker mount options, Web UI selectors, and tests.

## Operational Notes

- `just stack-up` uses the client-service image from `services/client_service_rs`.
- The Web UI is served at `http://127.0.0.1:8003`.
- `client_service` is available at `http://127.0.0.1:8002`.
- `agent_host` is available at `http://127.0.0.1:8001`.
- Workspace volume data persists outside AgentSpace records. Deleting a
  workspace through the API unregisters it only; it intentionally does not delete
  Docker/Podman volume contents.
- Session scratch volumes are temporary. They are mounted at `/workspace` for
  every kernel and are removed when the runtime session is destroyed unless the
  user saved them first.
- Running sessions do not update their mounts after agent config changes.
  Restart the session to pick up new workspace configuration.
