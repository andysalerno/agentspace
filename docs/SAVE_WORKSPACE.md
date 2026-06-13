# Save or Destroy Session Workspace Plan

## Goal

Every kernel session should start with a mounted `/workspace` volume, even when
the agent has no configured persistent workspaces. When the user ends the
session or kills the kernel, the UI should ask:

> Do you want to save this workspace, or destroy it forever?

If the user saves it, the current session workspace contents should become a new
named AgentSpace workspace. If the user destroys it, the session workspace volume
should be removed permanently.

## Recommended Filesystem Layout

Use `/workspace` as a per-session scratch volume and mount enabled persistent
workspaces directly under `/workspace/<workspace_id>`.

For every kernel session:

```text
/workspace/                 # per-session scratch volume, cwd
```

For a session with `todo-list-code` and `todo-list-items` enabled:

```text
/workspace/                 # per-session scratch volume, cwd
  todo-list-code/           # mounted Docker/Podman volume
  todo-list-items/          # mounted Docker/Podman volume
  app.py                    # scratch/session work
  notes.md                  # scratch/session work
```

Public/user-facing mount paths should still be reported as:

```text
/workspace/todo-list-code
/workspace/todo-list-items
```

## Why This Layout

Mounting persistent workspaces directly as nested volumes under `/workspace/<id>`
keeps all agent-accessible content inside the agent's allowed `/workspace` jail.
Docker volume contents mounted inside another volume are separate from the
parent volume, so save logic can exclude known mounted workspace names when
snapshotting the session scratch volume.

With direct mounts:

- saving `/workspace` copies normal session files only;
- mounted workspace data lives in separate volumes under `/workspace/<id>`;
- save logic can explicitly skip known mounted workspace names;
- the agent still sees enabled workspaces in its cwd;
- `KERNEL_ADDITIONAL_PATHS` can include `/workspace/<id>` so CLI harnesses can
  access mounted workspaces safely.

## Implementation Plan

### 1. Always Create a Session Scratch Volume in `agent_host`

Add a scratch volume concept to `services/agent_host_rs/src/docker_runtime.rs`.

Suggested volume name:

```text
agentspace-session-workspace-<session_id>
```

Mount it into every kernel container:

```text
/workspace
```

Use `rw` mode and labels such as:

```text
agentspace.role=session-workspace
agentspace.session_id=<session_id>
agentspace.managed=true
```

### 2. Mount Actual Enabled Workspaces Under `/workspace/<id>`

Keep client-visible mount paths as:

```text
/workspace/<workspace_id>
```

Mount persistent workspace volumes directly at the client-visible path. Add
`/workspace/<id>` to additional paths for harnesses that need explicit
filesystem allowlists.

Top-level names matching mounted workspace IDs are reserved for that session.

### 4. Add `agent_host` Snapshot/Save Support

Add an `agent_host` endpoint such as:

```text
POST /sessions/{session_id}/workspace/snapshot
```

Example payload:

```json
{
  "workspace_id": "saved-work",
  "volume_name": "agentspace-workspace-saved-work",
  "exclude_names": ["todo-list-code", "todo-list-items"]
}
```

The endpoint should:

1. Create the target persistent workspace volume.
2. Copy from the session scratch volume to the target volume.
3. Exclude mounted workspace names.
4. Avoid following symlinks.

Use a helper container with the kernel image and a small Python copy script
rather than `cp -a`, because Python can precisely avoid following symlink
targets.

Suggested copy behavior:

- copy regular files and directories from `/workspace`;
- skip top-level entries for enabled workspace IDs;
- do not follow symlinks;
- preserve user-created symlinks if safe, or exclude all top-level symlinks for
  a stricter first implementation.

### 5. Destroy Scratch Volumes on Session Destruction

`agent_host.destroy_session` should remove:

- the kernel container;
- the per-session scratch volume.

It should not remove persistent workspace volumes.

`destroy_all_sessions` should clean scratch volumes for in-memory sessions.
Startup cleanup for orphaned `agentspace-session-workspace-*` volumes can be a
separate safe cleanup pass.

### 6. Add Rust `client_service_rs` Save Orchestration

Add an `AgentHostClient` method such as:

```rust
snapshot_session_workspace(session_id, workspace_id, volume_name, exclude_names)
```

Add a Rust endpoint such as:

```text
POST /sessions/{session_id}/workspace/save
```

Example payload:

```json
{
  "workspace_id": "saved-work",
  "name": "Saved Work"
}
```

Flow:

1. Validate the client session exists.
2. Validate workspace ID format.
3. Ensure the workspace ID is not already registered.
4. Call `agent_host` snapshot with target volume name
   `agentspace-workspace-<workspace_id>`.
5. Insert a `WorkspaceRecord` in `client_service_rs`.
6. Return the new workspace summary.

Then the UI can call the existing delete-session endpoint to end and destroy the
scratch workspace.

### 7. Handle Save Atomicity

The main risk is partial failure between `client_service_rs` and `agent_host`.

Preferred robust shape:

1. Add a workspace status field such as `creating` / `ready`.
2. Create a `creating` record in Rust.
3. Snapshot into `agentspace-workspace-<id>`.
4. Mark the workspace `ready`.
5. Delete the session scratch volume only when the session is actually destroyed.

Simpler first implementation:

1. Snapshot first.
2. Insert the workspace record second.
3. If insert fails, call a best-effort `agent_host` cleanup endpoint for the
   newly created target volume.

The robust version is preferable if the schema change is acceptable.

### 8. Update Delete/End-Session UI

Replace the current direct delete confirmation in:

- `clients/webui/src/ChatView.tsx`
- `clients/webui/src/SessionsView.tsx`
- kernel kill flows in `clients/webui/src/KernelsView.tsx`

Use a reusable modal:

```text
Do you want to save this workspace, or destroy it forever?
```

Actions:

- **Save workspace**: show fields for workspace ID and display name, call the
  save endpoint, then delete the session/kernel.
- **Destroy forever**: call delete session/kernel directly.
- **Cancel**: do nothing.

Modal copy should explain:

```text
Mounted workspaces are linked into this workspace and will not be copied.
```

### 9. Handle Direct Kernel Kills

For kernels linked to a client session, route the user through the same
save/destroy dialog.

For orphan kernels with no client session, only offer destroy unless a later
agent-host-level raw snapshot UI is added.

The API-level `DELETE /kernels/{id}` can remain a force-kill/discard primitive,
but the Web UI should not call it without the prompt when a client session is
known.

### 10. Update Documentation

Update `docs/WORKSPACE_FEATURE.md` to describe:

- `/workspace` as the per-session scratch volume and cwd;
- `/workspace/<id>` as persistent volume mount targets;
- save/discard semantics.

## Test Plan

### `agent_host`

Add or update tests to verify:

- every kernel container gets a scratch volume mounted at `/workspace`;
- enabled persistent workspaces mount at `/workspace/<id>`;
- `additional_paths` includes mounted workspace paths;
- destroy removes the scratch volume;
- snapshot copies scratch files but excludes mounted workspaces.

### `client_service_rs`

Add tests to verify:

- the save endpoint validates workspace ID/name and duplicate IDs;
- the save endpoint calls `agent_host` snapshot with the correct target volume;
- the save endpoint excludes mounted workspace IDs;
- the new workspace is persisted and returned.

### Web UI

Add tests or manually verify:

- session delete opens the save/destroy modal;
- save path calls save endpoint before deleting;
- destroy path deletes without saving;
- kernel kill uses the same flow when a kernel is linked to a client session.

### Verification Commands

Run:

```sh
just client-service-rs-check
just agent-host-rs-check
npm --prefix clients/webui run lint
npm --prefix clients/webui run build
just check
```

## API Route Note

Do not rename the workspace management API routes. Keep:

```text
/workspaces
/workspaces/{workspace_id}
```

Those routes manage workspace records. Only the container filesystem layout
changes:

```text
/workspace                 # session scratch cwd
/workspace/<id>            # persistent volume mount
```
