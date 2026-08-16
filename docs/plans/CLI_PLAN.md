# CLI View Implementation Plan

## Status

Proposed implementation plan for the first CLI View release.

## Goal

Add a first-class **CLI** view to the web UI that runs the interactive GitHub
Copilot CLI inside an AgentSpace session container and renders that terminal in
the browser.

The first release must:

- use existing AgentSpace agent, connection, skill, workspace, secret, and
  session concepts;
- expose only agents explicitly configured for CLI use;
- support Copilot CLI even when that requires Copilot-specific behavior;
- provide a full terminal experience, including ANSI colors, Unicode and emoji,
  alternate-screen applications, mouse input, paste, and resize;
- allow multiple browsers to interact with the same live terminal; (note from PM: in fact, this is not a *requirement*, but it would be ideal. If making this functionality work requires a degree of sacrifice, it should be dropped in support of a more robust implementation)
- preserve the exact live terminal process and screen state across browser
  disconnects and service restarts while the session container remains alive;
- recover a dead Copilot process or container using Copilot's durable session
  ID and `copilot --resume=<id>`;
- keep the browser behind `client_service`; it must not connect directly to
  `agent_host`, Docker, or a kernel container;
- provide the same **VS Code** action as Chat; and
- leave the public API suitable for a future AgentSpace CLI client without
  implementing that client now.

## Explicit Non-Goals

- Supporting interactive CLIs other than Copilot CLI in the first release.
- Converting terminal output into Chat messages or storing a terminal transcript
  in `client_messages`.
- Preserving a live PTY across a host reboot. The Copilot session and workspace
  survive; the terminal process does not.
- Allowing Chat and CLI to drive the same Copilot session concurrently.
- Exposing arbitrary user-authored shell commands as CLI launch definitions.
- Building a general remote shell. The terminal endpoint launches only a
  configured, allowlisted agent CLI.
- Adding CLI View support to `clients/cli_ui` or `channels/cli_channel`.

## Current State and Gaps

The current layering is correct and should remain:

```text
Web UI -> client_service -> agent_host -> per-session kernel container
```

Relevant existing behavior:

- `client_service` owns the public API, durable SQLite session records, agents,
  connections, secrets, and configuration validation.
- `agent_host` owns Docker/container lifecycle but keeps its runtime session
  registry in memory.
- A kernel container already receives the resolved agent environment, skills,
  workspace mounts, a persistent `/workspace` volume, the shared Copilot config
  volume at `/root/.copilot`, and a code-server instance used by the VS Code
  action.
- Chat invokes Copilot in non-interactive prompt mode through `kernel_host`.
- Copilot Chat sessions already expose a resume token, but
  `client_service.SessionRecord` does not persist it.
- `agent_host` currently generates its own session ID, uses that ID for the
  container and workspace volume, and destroys all registered containers on
  graceful shutdown.
- There is no PTY abstraction, Docker exec/resize support, WebSocket transport,
  or terminal component in the web UI.
- The web UI's Nginx proxy handles streaming HTTP but does not yet forward
  WebSocket upgrade headers.

CLI View therefore needs a terminal execution path alongside the kernel event
path. It should reuse the session container and session configuration, not force
interactive terminal bytes through the existing `Kernel` event protocol.

## Core Design Decisions

### 1. Configure CLI support as an optional nested agent capability

Do not add three independent fields such as `allow_cli`, `cli_harness`, and
`cli_connection`. Their invalid combinations would complicate validation.
Instead, add an optional `cli` block; its presence means that the agent is
eligible for CLI View.

```yaml
apiVersion: agentspace.dev/v1alpha1
kind: Agent
metadata:
  name: reviewer
spec:
  harness: acp
  connection: openrouter
  systemPrompt: Review the current project.
  cli:
    harness: copilot-cli
    connection: openrouter
```

Proposed Rust model:

```rust
struct AgentCliConfig {
    harness: CliHarnessName,
    connection: Option<String>,
}

enum CliHarnessName {
    CopilotCli,
}
```

`CliHarnessName` must be separate from the broader kernel `HarnessName`. This
prevents the API from claiming that every kernel harness has an interactive CLI
implementation. Adding a future CLI requires an explicit launcher
implementation and an enum addition.

`cli.connection` references the existing `Connection` entity. It does not
duplicate a URL or API key. The field may be omitted to use Copilot's normal
GitHub authentication from the persistent Copilot config volume. In the UI,
enablement defaults the CLI connection picker to the agent's existing
`connection`, so OpenRouter and similar configurations can be shared by Chat
and CLI without being duplicated.

Required configuration changes:

- Add the nested field to `config/document.rs`, `AgentRecord`, projections in
  `config/adapter.rs`, canonical output, config-set loading, exports, bundle
  handling, and API request/response types.
- Validate that `cli.connection`, when present, resolves to a declared
  connection.
- Keep `deny_unknown_fields` behavior and canonical round trips strict.
- Reject any CLI harness other than `copilot-cli`.
- Show CLI eligibility, CLI harness, and CLI connection in Agents View.

### 2. Keep one logical session model, with an explicit interaction mode

`client_type` describes the caller (`webui` or `cli`) and must not be overloaded
to mean Chat versus CLI. Add:

```text
interaction_mode: chat | cli
harness_resume_token: nullable string
cli_harness: nullable enum
cli_connection_id: nullable string
```

The session's initial mode is supplied to `POST /sessions` and defaults to
`chat` for backward compatibility. A CLI session snapshots the selected CLI
harness and connection ID so later edits to the agent do not silently change
which provider the session is expected to resume with.

Secrets remain in the secret store and are resolved at launch/recovery time;
never persist resolved API keys in the session row.

The mode is not intended as a permanent partition. It records the active
session surface and gives v1 clear routing rules. A future transition endpoint
can stop one surface and resume the same harness token in the other. In v1:

- Chat message endpoints reject a CLI session with a clear conflict response.
- Terminal endpoints reject a Chat session.
- The UI lists Chat and CLI sessions in their respective views.
- Session deletion remains shared and destroys the terminal, container, and
  session workspace.

Cross-mode resume is only meaningful when both surfaces use the same underlying
harness and compatible provider configuration. The model must preserve that
information rather than promising cross-harness conversion.

### 3. Make the client session ID the stable runtime identity

Generate the durable `client_service` session ID before calling `agent_host` and
pass it as the requested runtime session ID. New `agent_host` session creation
must accept a caller-provided, validated ID and become idempotent:

- if the ID is registered, return the existing session;
- if its labeled container is running, adopt it into the in-memory registry;
- if the container is absent or stopped, recreate it with the same stable ID
  and session workspace volume; and
- reject a name/label collision rather than attaching to an unrelated
  container.

Use the full stable ID in Docker labels even if a shortened form remains in a
human-readable container name. Labels must include at least:

```text
agentspace.role=kernel
agentspace.session_id=<client session id>
agentspace.interaction_mode=cli
```

This removes the fragile durable mapping from a client session to a randomly
generated, in-memory-only host session. Existing Chat records remain readable;
new sessions use the stable identity, and a migration does not need to rewrite
old running sessions.

`client_service` must persist the session row in a recoverable `starting` state
before or transactionally around upstream creation. A failed upstream launch
must leave an explicit `error` state that can be retried or deleted, not a
success-shaped record.

### 4. Let a container-local multiplexer own the live PTY

Install `tmux` in the kernel image and use one private tmux session per
AgentSpace CLI session. Copilot runs as the tmux pane's foreground program.

This is preferable to keeping the only PTY master in a browser WebSocket or an
`agent_host` task:

- browser disconnects do not send EOF to Copilot;
- multiple browsers can attach as independent tmux clients and all can type;
- the PTY, terminal modes, cursor, alternate screen, and in-process application
  state survive `client_service` or `agent_host` restarts;
- a restarted `agent_host` can attach to the existing tmux session through
  Docker; and
- terminal resize remains a real PTY resize rather than an emulated escape
  sequence.

Configure a finite, generous tmux history limit and `remain-on-exit` so the last
screen and exit status remain inspectable. New attachments should receive the
current screen immediately; restore captured tmux history before the live
redraw if testing shows that normal attach does not provide usable scrollback.

Each browser WebSocket gets its own Docker exec running an allowlisted command
equivalent to:

```text
tmux attach-session -t <fixed-internal-name>
```

The exec has TTY, stdin, stdout, and stderr attached. Resize messages call the
Docker exec resize API. No user data is interpolated into a shell command.
Docker exec and tmux arguments are constructed as an argv vector.

`agent_host` needs a small terminal runtime interface, implemented first by the
Docker runtime:

```text
ensure_terminal(session, launch_spec) -> terminal summary
attach_terminal(session, initial_size) -> duplex byte stream + resize handle
terminal_summary(session) -> running/exited/missing metadata
stop_terminal(session)
```

This abstraction is deliberately about terminal lifecycle, byte transport, and
resize. It does not attempt to normalize arbitrary agent CLI semantics.

### 5. Give Copilot an explicit launcher implementation

Add a `CliLauncher` dispatch layer with one implementation:
`CopilotCliLauncher`. Do not expose an arbitrary command template in
configuration.

On the first launch:

1. Generate a UUID for the Copilot session in `client_service`.
2. Persist it as `harness_resume_token`.
3. Launch interactive Copilot with that known ID using
   `--session-id=<uuid>`.

On recovery after the prior Copilot process or container is gone:

```text
copilot --resume=<persisted uuid>
```

The launch spec should also include:

- `--no-auto-update`, so an interactive session cannot mutate the installed
  binary unexpectedly;
- `--mouse=on`;
- `--agent=<session-scoped profile>` when the AgentSpace agent has a system
  prompt;
- the configured model and reasoning effort;
- `--add-dir` for the same additional paths exposed to the kernel;
- `/workspace` as the working directory;
- existing validated Copilot extra arguments, represented as argv entries; and
- the same mounted authentication, skills, and workspace resources as Chat.

Copilot has no generic `--system-prompt` option. Materialize the AgentSpace
system prompt as a valid Copilot custom-agent profile and select it with
`--agent`. Give the profile a deterministic, collision-resistant name derived
from the stable AgentSpace session ID. Store it without secrets and update it
atomically. A user-level profile under `/root/.copilot/agents` is acceptable
because the name is session-specific; delete only that owned profile when the
AgentSpace session is deleted.

Do not synchronize per-agent enabled skills into the shared
`/root/.copilot/skills` directory: concurrent containers could remove or expose
one another's skill links. For CLI sessions, project the enabled skills into the
session-scoped `/workspace/.github/skills` directory, using links to the
read-only mounted skill staging area. Copilot discovers that documented
repository-level location automatically. Reconciliation must replace only
AgentSpace-owned links and preserve unrelated user content.

The launcher maps the selected existing connection into Copilot's documented
BYOK variables:

| AgentSpace value | Copilot CLI variable |
| --- | --- |
| connection URL | `COPILOT_PROVIDER_BASE_URL` |
| connection API key | `COPILOT_PROVIDER_API_KEY` |
| `chat_completions` | `COPILOT_PROVIDER_WIRE_API=completions` |
| `responses` | `COPILOT_PROVIDER_WIRE_API=responses` |
| configured model | `COPILOT_MODEL` |

The current Connection schema describes OpenAI-compatible endpoints, so v1 uses
`COPILOT_PROVIDER_TYPE=openai`. Anthropic/Azure support should wait for an
explicit provider type in the shared Connection model rather than URL guessing.
OpenRouter remains supported through its OpenAI-compatible endpoint.

Keep generic `CONNECTION_*` variables in the effective environment for other
kernel paths, but translate them in the Copilot launcher. Ensure logs redact
credentials and do not log the complete child environment.

Copilot-specific behavior is allowed here by design. Future launchers may reuse
the terminal runtime but must provide their own argv, environment mapping,
session-token semantics, and recovery behavior.

## Terminal Transport

### Public API

Proposed client-facing routes:

```text
POST /sessions
  { agent_id, interaction_mode: "cli", ... }

GET  /sessions/{session_id}
  includes interaction_mode, terminal summary, harness_resume capability

POST /sessions/{session_id}/terminal/ensure
  starts, adopts, or resumes the configured terminal

GET  /sessions/{session_id}/terminal
  returns current terminal status without attaching

GET  /sessions/{session_id}/terminal/ws
  WebSocket terminal attachment

POST /sessions/{session_id}/terminal/stop
  stops the live CLI but preserves the durable session token and workspace
```

Opening a CLI session in the UI calls `terminal/ensure`, then attaches. `ensure`
is idempotent so browser reloads and concurrent clients cannot launch duplicate
Copilot processes.

`client_service` implements the same routes internally against `agent_host` and
proxies the WebSocket in both directions. This keeps the API usable by a future
terminal client and preserves the rule that clients never call `agent_host`
directly.

### WebSocket framing

Use a small documented protocol rather than coupling the browser to a library's
private attach format:

- Client binary frame: UTF-8 encoded terminal input bytes.
- Client text frame:
  `{"type":"resize","cols":120,"rows":40}`.
- Server binary frame: raw PTY output bytes.
- Server text frames: structured lifecycle messages such as
  `ready`, `exited`, and `error`.
- WebSocket close: detach only; it must never imply stop.

Validate positive, bounded rows and columns before calling Docker. Put bounded
queues between each direction to prevent an unread browser or a blocked PTY
from consuming unbounded memory. On overflow, close that attachment with an
explicit error while leaving the tmux/Copilot session running.

Use binary output so arbitrary terminal bytes are not forced through JSON or
lossy UTF-8 conversion.

### Proxy and origin handling

- Enable Axum's `ws` feature in both Rust services.
- Add a WebSocket client dependency to `client_service` for the internal
  upstream connection.
- Reuse the configured browser-origin allowlist for WebSocket `Origin`
  validation.
- Update `clients/webui/nginx.conf.template` to forward `Upgrade` and
  `Connection` headers and use a terminal-appropriate idle timeout.
- Build the browser WebSocket URL from the current origin and `/api`; do not
  embed container or host ports.
- A dropped proxy connection is a detach. The browser may reconnect with
  bounded exponential backoff.

## Persistence and Recovery

There are two required persistence levels.

### Level 1: exact live terminal reattachment

The live hierarchy is:

```text
kernel container
  -> tmux server/session
    -> Copilot CLI process
```

The hierarchy is independent of every browser attachment. While the container
and tmux session are alive:

- closing a tab or browser changes nothing;
- another computer can attach to the same session;
- two attached browsers can both send input and receive output;
- restarting the web UI, `client_service`, or `agent_host` does not terminate
  the tmux session; and
- a newly started `agent_host` adopts the labeled container and attaches to the
  existing tmux session.

`agent_host` must no longer destroy kernel sessions merely because the service
receives a normal shutdown signal. Explicit session deletion remains
destructive. `just stack-down` already performs explicit labeled-container
cleanup; update it or add an explicit cleanup API so associated managed session
workspace volumes are also removed without making ordinary service restart
destructive.

### Level 2: durable Copilot resume

Persist the following in SQLite:

- stable AgentSpace session ID;
- agent ID;
- interaction mode;
- CLI harness and connection reference;
- Copilot session UUID;
- stable session workspace volume identity;
- current runtime generation/status; and
- timestamps.

The shared `/root/.copilot` volume contains Copilot's durable session data. The
stable session workspace volume contains working files. After a host reboot or
container loss:

1. CLI View loads the durable session.
2. `terminal/ensure` detects that no live tmux session exists.
3. `agent_host` recreates the container using the stable session workspace
   volume and current resolved secret values.
4. The Copilot launcher runs `copilot --resume=<uuid>`.
5. The browser attaches to the new tmux session.

The UI must distinguish **reattached live terminal** from **resumed Copilot
session** because only the former preserves exact terminal state.

If resume data, a connection, a secret, or the workspace volume is missing,
return a specific error and show it. Do not silently start a fresh Copilot
session under the old AgentSpace session.

### Startup reconciliation

Add reconciliation rather than assuming all durable `agent_host_session_id`
values are live:

- `client_service` keeps durable records authoritative for user-facing
  sessions.
- `agent_host` can adopt a running labeled container on demand.
- orphaned labeled containers without a client session remain visible for
  diagnostics and are removed only by explicit cleanup.
- a CLI session status is derived from durable state plus the current terminal
  summary, not from stale SQLite text alone.

## Web UI

### Navigation and layout

- Add `cli` to `ViewId` and place **CLI** directly after **Chat** in the primary
  sidebar.
- Implement `CliView.tsx` with the same broad two-pane shape as Chat:
  - left rail: CLI sessions and New Session;
  - main pane: header and terminal;
  - new-session dialog: only agents with `agent.cli != null`.
- Keep selected Chat and CLI session IDs separately in `App` so switching views
  does not lose either selection.
- Reuse existing session deletion, save-workspace, status, and browser URL
  helpers where behavior is identical.

### Terminal component

Use xterm.js, the terminal component used by VS Code:

```text
@xterm/xterm
@xterm/addon-fit
@xterm/addon-unicode11
@xterm/addon-webgl
```

Implementation requirements:

- Load `FitAddon` and call it from a `ResizeObserver`.
- Send the fitted rows/columns after attach and whenever they change.
- Load `Unicode11Addon` and select Unicode 11 width handling.
- Attempt `WebglAddon`, but fall back to the default renderer on unsupported
  devices or context loss.
- Configure a monospace stack with color-emoji fallback, a substantial
  scrollback limit, cursor behavior, and theme colors derived from the current
  AgentSpace light/dark theme.
- Pass xterm input to binary WebSocket frames and raw output bytes to
  `terminal.write`.
- Preserve normal xterm mouse reporting so Copilot's alternate-screen mouse
  support works.
- Dispose every xterm addon, listener, observer, timer, and socket on session
  change/unmount.
- Focus the terminal after a successful attachment, without stealing focus
  from dialogs.
- Provide accessible connection/status text outside the canvas and announce
  disconnect/error transitions. Do not claim that the terminal canvas itself
  is a complete screen-reader experience.

Do not use `@xterm/addon-attach`; the custom framing includes resize and
lifecycle control messages that the generic addon does not model.

### Header and recovery UX

The selected session header should show:

- agent name;
- live/resuming/exited/disconnected/error status;
- compact AgentSpace session ID;
- Copilot session ID in details;
- number of active attachments when available;
- **VS Code**, using the same `vscode_url` and
  `browserReachableLocalUrl` behavior as Chat;
- reconnect/resume action when automatic recovery needs user confirmation;
- save workspace; and
- stop/delete actions with distinct wording.

Closing or navigating away from CLI View only detaches. **Stop CLI** terminates
Copilot/tmux but preserves resumability. **Delete session** remains destructive
and follows the existing save-workspace prompt.

## Service Changes by Area

### `services/client_service_rs`

- Extend agent configuration and CRUD shapes with `cli`.
- Extend session models and SQLite schema using `ensure_column` migrations.
- Persist the Copilot session token and stable runtime identity.
- Generate the logical session/token before upstream creation.
- Resolve `cli.connection` through the existing resolver and secret store.
- Add terminal status/control routes and WebSocket proxying.
- Add clear conflict/not-found/recovery errors.
- Ensure agent deletion also stops and deletes CLI sessions.
- Keep terminal bytes out of application logs and SQLite.

### `services/agent_host_rs`

- Accept stable requested session IDs and make ensure/create idempotent.
- Add Docker container adoption based on validated labels.
- Add Docker exec attach, stdin/stdout streaming, and resize primitives.
- Add terminal/tmux lifecycle state to session summaries.
- Add the Copilot launcher and its connection/environment translation.
- Separate ordinary service shutdown from explicit session destruction.
- Preserve and reuse the session workspace volume during recovery; remove it
  only on session deletion.
- Add internal terminal HTTP/WebSocket routes.

### `kernels/kernel_host`

- Install `tmux` in the image.
- Keep the existing HTTP kernel host running in CLI containers so diagnostics,
  VS Code, workspace behavior, and future mode switching retain one container
  shape.
- Factor any Copilot argv/environment translation shared with
  `kernel_copilot`; do not allow Chat and CLI implementations to drift on model,
  connection, path, or resume semantics.

The terminal itself should not be implemented as a new `Kernel` protocol
method. The kernel protocol carries normalized agent events; a PTY is an
opaque, bidirectional byte stream with different lifecycle and backpressure
requirements.

### `clients/webui`

- Add xterm dependencies and styles.
- Add CLI types, queries, API/WebSocket helper, `CliView`, and reusable terminal
  component.
- Update Agents View CLI configuration controls.
- Update Sidebar, App routing, session navigation, and test fixtures.
- Update Nginx WebSocket proxy configuration.
- Extend the screenshot harness/mock API with CLI fixtures and a deterministic
  terminal state.

### Compose and operations

- Rebuild the kernel image with `tmux`.
- Keep the existing persistent Copilot config volume.
- Ensure `stack-down` removes intentionally abandoned CLI containers and their
  managed session workspace volumes, while service restart does not.
- Document that host reboot loses live terminal state but the next attach
  resumes Copilot.

## Implementation Sequence

### Phase 1: configuration and durable model

1. Add `CliHarnessName` and `AgentCliConfig`.
2. Wire the nested config through loaders, adapters, canonical YAML, CRUD, UI
   types, and validation.
3. Add session interaction mode, CLI snapshot fields, resume token, and SQLite
   migrations.
4. Extend session create/list/get contracts without changing default Chat
   behavior.

Exit criterion: an agent can be round-tripped with CLI configuration, and a CLI
session can be created durably in `starting` state without a terminal.

### Phase 2: stable runtime identity and reconciliation

1. Let `client_service` allocate and pass the stable session ID.
2. Make `agent_host` ensure/create idempotent.
3. Label containers and add safe adoption.
4. Reuse stable workspace volumes on recovery.
5. Remove destructive cleanup from ordinary `agent_host` shutdown and preserve
   explicit cleanup paths.

Exit criterion: restarting either Rust service can rediscover a still-running
session container, and recreating a missing container retains its workspace.

### Phase 3: Copilot terminal runtime

1. Add tmux to the image.
2. Add Docker exec/attach/resize support behind a tested runtime trait.
3. Implement `CopilotCliLauncher`, known session IDs, connection translation,
   first launch, live attach, stop, and resume.
4. Expose internal status/control/WebSocket routes.

Exit criterion: two terminal clients can interact with one Copilot process;
disconnecting both leaves it alive; deleting the session removes it.

### Phase 4: public transport

1. Add durable ensure/status/stop handlers to `client_service`.
2. Add the bounded WebSocket proxy and documented frame protocol.
3. Add origin validation, lifecycle errors, and Nginx upgrade handling.
4. Verify reattachment through both service restarts.

Exit criterion: a generic WebSocket client can create, attach, detach, reattach,
resize, and resume a CLI session using only `client_service`.

### Phase 5: web UI

1. Add CLI navigation, agent picker, session rail, header actions, and status
   states.
2. Add the xterm component, theme integration, resize, mouse, Unicode, WebGL
   fallback, reconnection, and cleanup.
3. Add VS Code and existing session/workspace actions.
4. Add unit and screenshot fixtures for light/dark and connected/disconnected
   states.

Exit criterion: the complete acceptance matrix below passes in the browser.

### Phase 6: hardening and documentation

1. Exercise process exit, container loss, Docker errors, stale records, missing
   secrets, and queue overflow.
2. Confirm credential redaction and strict command construction.
3. Document configuration, persistence semantics, operational cleanup, and the
   public terminal protocol.
4. Run the full repository verification and screenshot suite.

## Test Plan

### Configuration and storage

- Aggregate and per-resource YAML parse/serialize round trips for `cli`.
- Unknown CLI fields and unsupported CLI harnesses fail validation.
- Missing `cli.connection` references report the exact field path.
- Agent CRUD and config export retain CLI settings.
- SQLite upgrades an existing database without data loss.
- Existing sessions default to `interaction_mode=chat`.
- Resume tokens and CLI snapshot fields survive service restart.
- Resolved API keys never appear in agent/session JSON or SQLite session rows.

### Copilot launcher

- First launch uses `--session-id=<known uuid>`.
- Recovery uses `--resume=<same uuid>` and never silently generates a new ID.
- Connection URL/key/flavor map to the correct `COPILOT_PROVIDER_*` variables.
- No connection leaves Copilot in normal GitHub-auth mode.
- Model, effort, paths, mouse, and no-auto-update options are preserved.
- The AgentSpace system prompt is written to the selected Copilot custom-agent
  profile, and an empty prompt does not reuse a stale profile.
- Each CLI session sees exactly its enabled skills from its session-scoped
  `.github/skills` projection; concurrent sessions cannot alter that set.
- Arguments containing whitespace or punctuation remain single argv entries.
- Secrets and full environments are redacted from tracing.

### Terminal runtime and service contracts

- Attach sends input and receives raw output.
- Resize reaches the Docker exec PTY with bounded dimensions.
- Two simultaneous attachments both receive output and may send input.
- Dropping one or all WebSockets does not stop tmux/Copilot.
- Reattachment restores the live screen.
- `agent_host` restart adopts the running labeled container and tmux session.
- `client_service` restart uses its SQLite record and reattaches.
- Missing container starts a new tmux session with Copilot resume.
- Duplicate concurrent `ensure` calls launch at most one Copilot process.
- Stop preserves the resume token; delete removes container and workspace.
- Chat endpoints reject CLI sessions and terminal endpoints reject Chat
  sessions.
- Slow-client queue overflow detaches only that client.
- Invalid frames, origins, session IDs, and resize dimensions fail explicitly.

### Web UI

- CLI agent picker excludes agents without CLI configuration.
- New CLI session uses `interaction_mode=cli`.
- xterm writes binary output and sends input/resize frames.
- Session changes dispose the old socket, terminal, observers, and timers.
- Reconnect backoff stops after successful attachment or component unmount.
- Dark/light themes update terminal colors.
- WebGL failure falls back without losing the session.
- VS Code uses the selected runtime's browser-reachable URL.
- Navigating away detaches but does not call stop.
- Stop and Delete have different confirmation and API behavior.

### End-to-end acceptance matrix

1. Start a Copilot CLI session through an agent using an OpenRouter connection.
2. Verify colors, emoji, paste, mouse interaction, alternate screen, and resize.
3. Open the same session in two browsers and type from both.
4. Close both browsers, reopen from another machine, and observe the exact live
   terminal.
5. Restart web UI and `client_service`; reattach without restarting Copilot.
6. Restart `agent_host`; adopt the existing container/tmux session and reattach.
7. Kill the Copilot process or session container; reopen and verify
   `--resume=<persisted id>` recovery.
8. Reboot the host; verify live PTY state is reported lost, then resume the same
   Copilot session with the same workspace.
9. Open VS Code from CLI View and confirm it targets the same session
   container/workspace.
10. Delete the session and verify the terminal, container, and managed session
    workspace are removed.

## Validation Gates

Every implementation phase must keep its affected targeted checks green. Before
the feature is complete, run:

```text
just agent-host-check
just client-service-check
just webui-lint
just webui-screenshots
just check
```

No phase may weaken strict Rust linting, TypeScript checks, config validation,
or existing Chat behavior to make the terminal path pass.

## Principal Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Browser disconnect kills the CLI | tmux owns the live PTY; WebSockets are attachments only. |
| Service restart loses runtime registry | Stable IDs, Docker labels, idempotent ensure, and container adoption. |
| Host reboot loses exact screen | State the boundary clearly and recover through persisted Copilot ID/workspace. |
| Duplicate recovery starts two Copilot processes | Per-session async lock around ensure/start/adopt. |
| Terminal proxy becomes a remote shell | Allowlisted launcher and fixed tmux attach argv only; no configured shell command. |
| Secrets leak through persistence/logs | Resolve at launch, persist references only, redact env and frames. |
| One slow browser exhausts memory | Bounded per-attachment queues and explicit disconnect on overflow. |
| Multiple browser sizes fight | Let each tmux client report its PTY size and configure a documented tmux window-size policy; test mixed sizes. |
| Connection semantics differ by CLI | Keep connection entities shared but put translation in the Copilot launcher. |
| Chat/CLI resume becomes impossible | Persist a harness resume token independently of UI mode and prohibit concurrent drivers. |
| xterm/WebGL is unsupported | Use the default renderer as a tested fallback. |

## Definition of Done

CLI View is complete when a CLI-enabled agent can start Copilot through an
existing Connection, multiple browsers can share and reattach to its live
terminal, a lost process/container/host can resume the same durable Copilot
session, VS Code opens the same workspace, all destructive and recovery states
are explicit, and the full validation and acceptance suites pass without
regressing Chat.
