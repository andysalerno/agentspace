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
- `agentspace-skills` and `agentspace-memory-data`; and
- any separately managed workspace or skill-resource volumes.

The exact live CLI pane, process, screen, and tmux scrollback exist only in the
running kernel container. Browser disconnect and service restart preserve that
container. Container removal or host reboot loses the exact screen, but
terminal ensure recreates the runtime with the same Copilot UUID and durable
workspace/state volumes. Missing recovery volumes or required configuration
produce an explicit conflict rather than a replacement UUID.

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
role labels. It removes only unowned managed kernel containers and
session-workspace volumes. Durable sessions retain their workspace volumes.

`just stack-down` first asks `client_service` to remove managed orphans, then
stops Compose, then removes both running and stopped dynamic containers that
carry the exact managed kernel/gateway labels. It does not use name prefixes,
remove unrelated volumes, or run broad container/volume deletion. If the
cleanup API is unavailable, it retains session volumes and prints a warning
rather than guessing ownership.

Explicit session deletion is destructive: it detaches clients and removes that
session's managed container and session-workspace volume. Save the workspace
first when its files must outlive the session.

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
