# Local operations

AgentSpace is intended for a trusted, single-user environment. Compose
publishes the Web UI on `0.0.0.0` for access from a trusted local network and
publishes the direct `client_service` API on `127.0.0.1` by default. There is
no application authentication and no TLS requirement or certificate setup for
this trusted deployment. Do not expose the Web UI or API to an untrusted
network without adding authentication, authorization, and TLS in front of the
stack. Override the bindings with `AGENTSPACE_WEBUI_BIND_HOST` and
`AGENTSPACE_CLIENT_SERVICE_BIND_HOST`.

## Persistent data

Back up these resources together:

- `mounts/data/client_service/client_service.sqlite`: agents, durable sessions,
  configuration, and encrypted secret values;
- `CLIENT_SERVICE_SECRET_KEY`: required to decrypt those values;
- `agentspace-kernel_copilot-config`: Copilot authentication and durable
  Copilot session state;
- labeled `agentspace-session-workspace-*` volumes: per-session working files;
- labeled `agentspace-session-telemetry-*` volumes: per-session Copilot
  telemetry JSONL and the normalized checkpoint for CLI sessions that have
  managed telemetry enabled;
- `agentspace-skills` and `agentspace-memory-data`; and
- any separately managed workspace or skill-resource volumes.

The exact live CLI pane, process, screen, and tmux scrollback exist only in the
running kernel container. Browser disconnect and service restart preserve that
container. Container removal or host reboot loses the exact screen, but
terminal ensure recreates the runtime with the same Copilot UUID and durable
workspace/state volumes. Missing recovery volumes or required configuration
produce an explicit conflict rather than a replacement UUID.

CLI telemetry is intentionally kept out of `/workspace`, so workspace snapshot
and clone operations do not capture it. If you need historical CLI telemetry
after deleting or recreating a session, include the matching
`agentspace-session-telemetry-*` volume in backups. See
[`docs/TELEMETRY_PROTOCOL.md`](./TELEMETRY_PROTOCOL.md) for the normalized
model and route contract.

## CLI telemetry volumes

New durable CLI sessions currently store
`telemetry_volume_identity = <session_id>`. `agent_host` uses that stable
identity to derive a deterministic Docker volume name:

```text
agentspace-session-telemetry-<first 12 chars of telemetry_volume_identity>
```

The volume is mounted read-write into the kernel container at:

```text
/var/lib/agentspace/telemetry
```

Its labels are intentionally minimal:

```text
agentspace.managed=true
agentspace.role=session-telemetry
agentspace.session_id=<full durable session ID>
```

Unlike session-workspace volumes, telemetry volumes deliberately do **not** use
`agentspace.interaction_mode` or `agentspace.resource_id` labels.

Lifecycle rules:

- session creation creates or adopts the labeled telemetry volume when
  `telemetry_volume_identity` is present;
- **Stop CLI** stops tmux but keeps the telemetry volume;
- runtime adoption and recovery remount the same telemetry volume when its
  labels still match the durable session;
- recovery fails explicitly if a session expects a telemetry volume and that
  durable volume is missing; and
- explicit session deletion removes the managed container, workspace volume, and
  telemetry volume together.

Legacy or migrated rows may still have `telemetry_volume_identity = null`
(`None` in code). That means no managed telemetry history exists for that
session. It does **not** mean the session workspace is broken or unrecoverable,
and AgentSpace does not invent a telemetry volume for it during recovery.

## Telemetry health, bounds, and privacy

CLI telemetry is auxiliary. A degraded or unavailable telemetry snapshot does
not stop the terminal or PTY transport.

Current reader bounds are enforced inside `kernel_host`:

- 256 managed JSONL files;
- 64 MiB of unread raw telemetry bytes per snapshot pass;
- 512 KiB per JSONL line;
- 50,000 distinct normalized spans;
- an 8 MiB compressed checkpoint file; and
- 64 MiB of uncompressed checkpoint data while loading the compact checkpoint.

When those bounds are exceeded, or when the reader hits malformed data,
duplicate conflicts, checkpoint problems, or content-policy conflicts,
`/sessions/{session_id}/telemetry` reports warnings and typically
`state=degraded`. `state=starting` means the managed source exists but no
completed model call has been normalized yet. `state=unavailable` means the
session has no managed telemetry identity, has not been recovered yet, or its
runtime is not currently inspectable.
Upstream telemetry provider failures surface as `503` responses instead. The
browser may also show `stale` when it is holding a previous successful snapshot
during polling retries.

The raw telemetry files are metadata-only by policy, but they still contain
sensitive operational metadata such as model/provider/tool names, IDs,
timestamps, token counts, and context occupancy. The same agent shell that can
write the workspace can also read, delete, or forge telemetry inside the kernel
container. Treat telemetry volumes as sensitive backups and as agent-reported
operational data, not tamper-evident billing evidence.

## Restart and adoption

`client_service` is the durable authority. `agent_host` deliberately forgets
its in-memory registry on shutdown without deleting kernel containers. After a
service restart, the next ensure adopts only containers and volumes whose full
labels match the durable session. Stale or conflicting labels are rejected.

Simultaneous terminal attachments are supported. Disconnecting every client
does not stop Copilot; use **Stop CLI** or the terminal stop route explicitly.

## Cleanup

Preview orphan cleanup:

```sh
curl --fail --silent --show-error \
  -H 'content-type: application/json' \
  -d '{"dry_run":true}' \
  http://127.0.0.1:8002/management/runtime-cleanup | jq
```

Apply the reported cleanup:

```sh
curl --fail --silent --show-error \
  -H 'content-type: application/json' \
  -d '{"dry_run":false}' \
  http://127.0.0.1:8002/management/runtime-cleanup | jq
```

Cleanup considers full `agentspace.session_id`, `agentspace.managed=true`, and
role labels. It removes only unowned managed kernel containers,
session-workspace volumes, and session-telemetry volumes. Durable sessions
retain their owned workspace and telemetry volumes.

`just stack-down` first asks `client_service` to remove managed orphans, then
stops Compose, then removes both running and stopped dynamic containers that
carry the exact managed kernel/gateway labels. It does not use name prefixes,
remove unrelated volumes, or run broad container/volume deletion. If the
cleanup API is unavailable, it retains session volumes and prints a warning
rather than guessing ownership.

Explicit session deletion is destructive: it detaches clients and removes that
session's managed container, session-workspace volume, and session-telemetry
volume. Save the workspace first when its files must outlive the session, and
back up telemetry first when its historical usage metadata must survive.

## Safe telemetry diagnostics

Prefer normalized APIs and volume labels over reading raw JSONL files.

Inspect only the durable session linkage:

```sh
curl --fail --silent --show-error \
  http://127.0.0.1:8002/sessions/<session_id> \
  | jq '{session_id,interaction_mode,recovery_state,runtime_status,telemetry_volume_identity}'
```

Inspect normalized telemetry health without exposing raw payloads:

```sh
curl --fail --silent --show-error \
  http://127.0.0.1:8002/sessions/<session_id>/telemetry \
  | jq '{state,reason,content_mode,source_version,observed_at,received_at,reporting,warnings}'
```

Preview orphan cleanup for telemetry volumes as well as containers and workspace
volumes:

```sh
curl --fail --silent --show-error \
  -H 'content-type: application/json' \
  -d '{"dry_run":true}' \
  http://127.0.0.1:8002/management/runtime-cleanup \
  | jq '.resources[] | select(.kind=="session_telemetry_volume")'
```

Confirm volume ownership labels without printing telemetry contents:

```sh
docker volume inspect agentspace-session-telemetry-<suffix> \
  --format '{{json .Labels}}'
```

Replace `docker` with `podman` where appropriate. Avoid `cat`, `tail`, or other
raw-file inspection in routine diagnostics unless you are intentionally handling
sensitive metadata.

## Container-gated terminal validation

Run the opt-in kernel/tmux/PTY flow with a reachable Podman or Docker daemon:

```sh
just terminal-container-integration
```

The script builds the current kernel image, creates uniquely named and labeled
test resources, and validates ensure, duplicate ensure, PTY I/O/resize, two
clients, detach/reattach, dead-pane resume, container recovery, stable Copilot
identity, and workspace/state persistence. It skips cleanly when no compatible
daemon is reachable and deletes only resources carrying its unique test label.

Set `AGENTSPACE_TERMINAL_INTEGRATION_SKIP_BUILD=1` to use an already-built
image, or `CONTAINER_RUNTIME=podman|docker` to select a daemon explicitly.
