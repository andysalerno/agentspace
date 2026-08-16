# Terminal protocol

AgentSpace exposes persistent CLI sessions through `client_service`. Browser
and future non-browser clients use the same HTTP and WebSocket contract; they
must not call `agent_host` or `kernel_host` directly.

## Public HTTP routes

All routes are under `/sessions/{session_id}/terminal` on `client_service`.

| Method and route | Purpose |
| --- | --- |
| `GET /terminal` | Observe terminal state and attachments. |
| `POST /terminal/ensure` | Create, adopt, or recover the terminal. |
| `POST /terminal/stop` | Stop tmux without deleting the durable session. |
| `POST /terminal/resume` | Respawn an exited pane with the durable Copilot ID. |
| `GET /terminal/ws` | Upgrade to a raw terminal attachment. |

The status/control response includes `state` (`missing`, `running`, or
`exited`), `exit_status`, `attach_kind` (`started`, `attached`, or `resumed`),
and attachment count. Internal socket paths, attach commands, pane PIDs, and
tmux client records are never exposed by `client_service`.

The Web UI implements scrollback in its own xterm buffer. It does not mutate
tmux copy mode, so one browser attachment cannot change another attachment's
pane interaction state.

`client_service` proxies equivalent internal routes on `agent_host`.
`agent_host` uses `kernel_host`'s `/terminal`, `/terminal/ensure`,
`/terminal/stop`, `/terminal/resume`, and `/terminal/detach-client` routes for
control only. PTY bytes never travel over those internal HTTP routes.

## WebSocket frames

Client to server:

- **Binary:** raw terminal input bytes. No UTF-8 or JSON transformation occurs.
- **Text:** exactly `{"type":"resize","cols":120,"rows":40}`.
- Ping, pong, and close frames retain normal WebSocket meaning.

Server to client:

- **Binary:** raw Docker-exec PTY output bytes.
- **Ready text:** sent once after attachment:

  ```json
  {
    "type": "ready",
    "attachment_id": "uuid",
    "cols": 80,
    "rows": 24,
    "terminal": {"state": "running", "attach_kind": "attached"}
  }
  ```

- **Exited text:** `type`, `state`, `exit_status`, and the latest `terminal`
  status. It is followed by a normal close.
- **Error text:** `{"type":"error","code":4429,"message":"..."}` followed by a
  close frame with the same application code.

Each WebSocket is one attachment, not the terminal process. Closing it detaches
that tmux client only. Multiple clients can attach simultaneously, receive the
same pane output, and send input. Tmux uses its `window-size smallest` policy
when client dimensions differ.

## Errors and close codes

Errors detected before upgrade are ordinary JSON HTTP errors:

- `403` for a browser `Origin` outside the configured allowlist;
- `404` for an unknown durable session;
- `409` for the wrong interaction mode, legacy/non-recoverable records,
  inconsistent snapshots, unavailable required recovery state, or a terminal
  that is not running;
- `422` for invalid control payloads; and
- `503` when `agent_host`, the kernel controller, or the container runtime is
  unavailable.

After upgrade, these close codes are used:

| Code | Meaning |
| --- | --- |
| `1000` | Normal detach or completed attachment. |
| `1011` | Internal terminal transport failure. |
| `4404` | Durable session was removed. |
| `4409` | Terminal state or attachment changed and the client must re-observe it. |
| `4429` | This attachment exceeded a queue or forwarding bound. |
| `4503` | Upstream terminal service or PTY became unavailable. |

Close reasons are diagnostic and bounded; clients must branch on the code.

## Bounds and backpressure

- WebSocket messages and frames are limited to 1 MiB at both public proxy
  boundaries.
- Rows and columns must each be in `1..=1000`.
- `agent_host` uses bounded per-attachment input and output queues (64 entries
  by default).
- Public and host WebSocket sends have five-second forwarding limits.
- A slow or overflowing client receives `4429`; tmux and other clients remain
  attached.

## Reconnect and persistence

The Web UI retries abnormal transport loss (`1006`, `1011`, `4429`, and
`4503`) up to five times with bounded exponential backoff. It does not retry a
normal detach, a missing session, a state conflict, an exited lifecycle frame,
or after unmount.

The exact live terminal (pane, screen, scrollback, and process) persists only
while the kernel container and tmux server survive. Browser disconnect,
Web UI restart, and Rust service restart do not stop it; `agent_host` adopts
the labeled running container and removes stale tmux clients.

Container or host loss destroys the exact live screen. Recovery creates a new
tmux session with the same durable Copilot UUID, session-workspace volume, and
Copilot state volume, and reports `attach_kind=resumed`. If required
configuration, secrets, Copilot state, or the durable session workspace is
missing, ensure fails instead of silently changing the Copilot UUID.

## Trust model and other clients

The terminal is a remote shell-equivalent capability. AgentSpace has no user
authentication, and the `Origin` check is browser hardening, not
authentication. Compose exposes the Web UI to the trusted local network while
keeping the direct API port on loopback by default. A trusted local deployment
does not require TLS or locally generated certificates.

Do not expose the API or Web UI to an untrusted network. A remote deployment
must add real authentication, authorization, and TLS at a trusted reverse
proxy.

Non-browser clients may omit `Origin`, use the same routes, preserve binary
frames byte-for-byte, parse lifecycle text frames, honor bounds and close
codes, and re-observe status before reconnecting.
