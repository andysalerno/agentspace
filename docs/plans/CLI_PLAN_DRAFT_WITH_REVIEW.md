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

## Review of Proposed Design

Reviewer notes appended after an independent read of the plan and verification
against the current tree and the installed Copilot CLI. Nothing above this
section was modified.

### Summary judgment

The core architecture is sound and should proceed. The decision to let a
container-local multiplexer own the PTY, to make the durable `client_service`
session ID the stable runtime identity, and to keep terminal bytes out of the
`Kernel` event protocol are all correct and are the decisions that matter most.
The plan is unusually specific about failure modes, which is welcome.

The issues below are concentrated in three places: the security story is
weaker than the plan claims, one component boundary is not implementable as
written, and several small pieces of state ("did we launch before?", "is the
pane alive?", "who is attached?") are inferred where they should be observed.

### Verification of the plan's factual claims

Claims checked against the tree at review time:

| Claim | Result |
| --- | --- |
| Shared Copilot config volume mounted at `/root/.copilot` | Confirmed (`docker_runtime.rs`, `AGENT_HOST_COPILOT_VOLUME`) |
| `agent_host` registry is in-memory only and generates its own UUID | Confirmed (`sessions.rs`) |
| `agent_host` destroys all sessions on graceful shutdown | Confirmed (`main.rs` -> `AppState::shutdown` -> `destroy_all_sessions`) |
| No PTY, Docker exec/resize, or WebSocket support today | Confirmed; `bollard` is the Docker client and does support exec + resize |
| Axum `ws` feature not enabled in either service | Confirmed |
| Only `agentspace.role=kernel` is labeled; no adoption logic | Confirmed |
| `ensure_column` migration helper exists | Confirmed (currently used only for `workspaces.status`) |
| Nginx template does not forward `Upgrade`/`Connection` | Confirmed |
| A resume token already surfaces from the runtime but is not persisted by `client_service` | Confirmed |
| Copilot flags `--session-id`, `--resume`, `--agent`, `--add-dir`, `--mouse`, `--no-auto-update`, `--effort` | All confirmed present |
| `COPILOT_PROVIDER_*` / `COPILOT_MODEL` BYOK mapping | Confirmed correct against `copilot help providers` |

Two corrections:

1. The plan describes the shared-skills hazard as a CLI concern. It is a
   **pre-existing defect that affects Chat today**: `skills_mount_path` returns
   `/root/.copilot/skills` for the `copilot-cli` harness, that path lives inside
   the shared Copilot volume, and `kernel_host` symlinks enabled skills into it
   at startup. Concurrent Chat containers already race here. Fixing this only
   for CLI would violate the plan's own "do not let Chat and CLI drift" rule on
   day one. Fix it for both surfaces, or state explicitly that Chat keeps the
   broken behavior for now.
2. `COPILOT_PROVIDER_WIRE_API` defaults to `completions`, not `responses`. The
   mapping table is still correct; just note that the `chat_completions` case is
   a no-op default and should be set explicitly anyway for clarity.

### Substantive concerns

Ordered by how expensive they are to fix late.

#### 1. The terminal is a remote shell. The non-goal is not achievable as stated.

"Building a general remote shell" is listed as a non-goal, and the mitigation
table claims the allowlisted `tmux attach-session` argv prevents it. It does
not. There are two independent escapes:

- tmux's prefix key is available to the attached client; `C-b :` then
  `new-window` yields an arbitrary shell in the container.
- Copilot CLI itself has a bash tool and will be launched with broad
  permissions. Anyone who can open CLI View can run commands in that container
  regardless of what tmux allows.

Recommendation: delete the claim and replace it with an accurate statement of
the trust boundary — *any principal who can open a CLI session has command
execution inside that session's container*. Then harden what is cheap to
harden: launch tmux with a dedicated locked config (`-f`), `unbind-key -a` on
both key tables, and no prefix. That removes the accidental escape and the
"I broke the pane layout" support burden, without pretending the container is
sandboxed from its own user.

#### 2. There is no authentication on the transport, only an origin allowlist.

`client_service` currently has a CORS origin allowlist and no authentication.
The plan carries that forward, reusing "the configured browser-origin
allowlist for WebSocket `Origin` validation" as the only stated control on an
endpoint that grants container command execution. Origin validation is a CSRF
control; it authenticates nothing.

This is tolerable for the rest of the API today and is materially less
tolerable for this endpoint. It also has a concrete design consequence:
browsers cannot set request headers on a WebSocket handshake, so a bearer token
cannot simply be added later without reworking the client.

Recommendation: decide the mechanism now, even if the initial implementation is
a no-op. The cleanest browser-compatible option is a **single-use, short-TTL
attach ticket**: `POST /terminal/ensure` returns a ticket, the WS URL carries it
as a query parameter, `client_service` consumes it on upgrade. It is
revocable, auditable, survives a future auth system, and avoids putting a
long-lived credential in a URL. If a ticket is judged premature, say so
explicitly in the plan and note that the Nginx access log must not record the
query string.

#### 3. The `CopilotCliLauncher` boundary is not implementable as written.

The plan places `CopilotCliLauncher` in `agent_host` (Rust) and simultaneously
instructs `kernel_host` to "factor any Copilot argv/environment translation
shared with `kernel_copilot`" (Python). These cannot both happen; the
translation cannot be shared across the language boundary, so it will be
duplicated and will drift on model, effort, `--add-dir`, and resume semantics —
precisely the drift the plan is trying to prevent.

Recommendation: move launch-spec construction into `kernel_host`, exposed as a
small internal HTTP endpoint (`POST /terminal/ensure`, `GET /terminal`).
`agent_host` then owns only the generic, Copilot-agnostic terminal runtime:
exec-attach, byte transport, resize, and lifecycle. This:

- puts all Copilot argv/env knowledge in one Python module next to
  `kernel_copilot`, where it can genuinely be shared;
- does **not** add a `Kernel` protocol method, so it respects the plan's own
  (correct) rule that PTY bytes are not kernel events — `kernel_host`'s HTTP API
  is not the `Kernel` protocol;
- keeps the `ensure_terminal` / `attach_terminal` / `terminal_summary` /
  `stop_terminal` interface intact, just with a thinner `ensure_terminal`; and
- makes idempotency a local, atomic problem (see next item).

#### 4. Prefer tmux's atomicity over a per-session async lock.

The risk table mitigates duplicate recovery with "per-session async lock around
ensure/start/adopt". A lock in `agent_host` process memory does not survive an
`agent_host` restart mid-`ensure`, which is exactly the scenario the plan cares
about.

`tmux new-session -d -s <name>` already fails atomically inside the tmux server
when the session exists. Making `ensure` unconditionally attempt creation and
treat "duplicate session" as success gives correctness that does not depend on
any in-memory state. Keep the lock as a latency optimization, not as the
correctness argument.

#### 5. `--session-id` alone removes the first-launch/recovery state machine.

The plan branches: `--session-id=<uuid>` on first launch, `--resume=<uuid>` on
recovery. The installed CLI documents `--session-id <id>` as "Resume an
existing session or task by ID, **or** set the UUID for a new session". One
flag covers both.

This matters because the branch is driven by persisted state ("have we launched
before?") that can be wrong after a crash between generating the UUID and the
process actually starting. In that window the plan would run
`--resume=<uuid>`, which hard-errors and exits — and under `remain-on-exit`
the pane stays alive, so `ensure` would report success while the user stares at
a dead pane. Always passing `--session-id` eliminates the state, the branch,
and that failure mode. Recommend dropping `--resume` from the design entirely.

#### 6. Liveness must be observed, not inferred.

`remain-on-exit on` is the right choice, but it means a live tmux session no
longer implies a live Copilot process; `has-session` becomes useless as a
health check. The UI states the plan promises (`live` / `exited` / `error`)
cannot be derived from what `terminal_summary` currently proposes to return.

Recommendation: `terminal_summary` should read pane state directly, e.g.
`tmux list-panes -F '#{pane_dead} #{pane_dead_status} #{pane_pid}'`, and return
a three-way `running` / `exited(status)` / `missing`. Also add
`attach_kind: "started" | "attached" | "resumed"` to the `ensure` response and
to the WS `ready` frame — the Persistence section requires the UI to
distinguish a reattached live terminal from a resumed Copilot session, but no
proposed field carries that information.

#### 7. Two scrollback buffers will fight; pick one now.

xterm.js maintains its own scrollback, which will be empty after every
reattach, while tmux holds the real history. The plan hedges ("restore captured
tmux history before the live redraw if testing shows..."), but that hedge is
actively wrong when Copilot is in the alternate screen: `capture-pane` returns
the alt screen, and replaying it before attach double-draws.

Recommendation: decide now that **tmux owns history**. Set xterm
`scrollback: 0`, do not replay `capture-pane`, and expose scrollback through an
explicit UI control that enters tmux copy-mode. This is one coherent model
instead of two competing ones, and it removes a whole class of "the terminal
duplicated my output" bugs.

#### 8. Mouse ownership is unresolved.

`--mouse=on` (Copilot) and tmux `mouse on` both want wheel and click events, and
the plan asks for both scrollback-by-wheel and full alternate-screen mouse
support without saying who wins. Recommendation: tmux `mouse off`, Copilot
`--mouse=on`. That matches a local Copilot session exactly, which is the stated
goal, and it composes with item 7. Make this an explicit tested decision rather
than something discovered during Phase 5.

#### 9. Locale and character width are unaddressed.

The kernel image is `python:3.14-slim` and sets no `LANG`/`LC_ALL`. tmux is not
UTF-8 safe without a UTF-8 locale (or `-u`), and the design stacks three
independent width tables — Copilot's, tmux's, and xterm's Unicode 11 addon. If
they disagree, emoji and box drawing smear and the cursor desynchronizes, which
is the single most visible failure mode for this feature.

Recommendation: add `ENV LANG=C.UTF-8` alongside the tmux install, pin the tmux
version, and promote emoji/CJK width to a named acceptance test with a fixed
sample string rather than the current "verify colors, emoji, paste".

#### 10. Orphaned execs and phantom tmux clients need reconciliation.

Every attachment creates a Docker exec that nothing explicitly reaps. If
`agent_host` dies mid-stream, the exec survives holding a dangling stream and
tmux keeps a phantom client — which continues to constrain window size under
any `window-size` policy other than `latest`. Nothing in the plan removes them,
and the header's "number of active attachments" has no source.

Recommendation: add periodic reconciliation of `tmux list-clients` against
known attachments with `detach-client` for the strays, set
`destroy-unattached off`, and source the attachment count from that
reconciliation.

#### 11. Phase 2 is a disk-growth regression without a cleanup path.

Removing destructive shutdown from `agent_host` is correct for this feature but
changes operational behavior for **Chat** as well: every ordinary service
restart now leaks a container and a session-workspace volume. Today
`stack-down` removes only *running* containers labeled `agentspace.role=kernel`
and removes no volumes at all, so the leak is currently unbounded and silent.

The plan acknowledges this in one sentence. It deserves concrete scope in
Phase 2: remove exited kernel containers, remove `agentspace.managed=true`
session-workspace volumes with no owning client session, and provide either an
age/idle reaper or an explicit cleanup API. Otherwise Phase 2 ships a
regression that Phase 6 is expected to notice.

#### 12. Volume labels key off the wrong identity.

Session-workspace volumes are named from the first 12 characters of the session
ID and labeled `agentspace.container_name=<container>`. Under the new stable-ID
scheme, adoption and garbage collection would key off a truncated,
collision-prone value. Relabel volumes with `agentspace.session_id=<full id>`
and treat the truncated name purely as a display detail.

Relatedly: the plan says migration "does not need to rewrite old running
sessions", which is reasonable, but the consequence is that pre-migration
sessions are permanently unadoptable. Say so explicitly and make it a supported
non-error state in the UI rather than something that surfaces as a recovery
failure.

#### 13. Secrets are readable from inside the terminal.

"Resolve at launch, persist references only, redact logs" does not cover the
new exposure: `COPILOT_PROVIDER_API_KEY` sits in the container environment and
the session's user has command execution (item 1). Copilot CLI provides
`--secret-env-vars=<names>`, which strips values from shell and MCP child
environments and redacts them from output. Use it for the provider key and any
`CONNECTION_*` duplicates, and state plainly in the plan that the container
environment is not a secrecy boundary against the session's own user.

#### 14. Put the generated agent profile in `.github/agents`, for the same reason as skills.

The plan moves skills to `/workspace/.github/skills` to escape the shared
volume, then leaves the generated system-prompt profile in shared
`/root/.copilot/agents` with a collision-resistant name and a bespoke
"delete only that owned profile" cleanup path. `.github/agents` is an equally
documented repo-level location. Using it makes profile lifetime identical to
the session workspace, removes the collision concern, and deletes the cleanup
path entirely. One caveat to resolve: decide whether AgentSpace-owned files
under `.github/` are excluded from **save workspace**.

#### 15. Chat has no BYOK translation today.

`kernel_copilot` passes `COPILOT_MODEL` and generic `CONNECTION_*` and never
sets `COPILOT_PROVIDER_*`. CLI would therefore be the first path where a
Connection actually takes effect for the `copilot-cli` harness. Given the
plan's explicit anti-drift requirement, this should be named as either in scope
or out of scope; silence guarantees that Chat and CLI disagree about which
model answered.

### Smaller notes

- The WS framing says "Client binary frame: UTF-8 encoded terminal input
  bytes." Terminal input is not necessarily valid UTF-8 (raw key sequences,
  pasted binary). Specify **raw bytes** in both directions.
- Define WebSocket close codes for queue overflow, origin rejection, auth
  failure, and session-gone. The plan requires distinct client behavior for
  these but gives the client no way to tell them apart.
- Snapshot the resolved model and reasoning effort onto the session alongside
  `cli_harness` and `cli_connection_id`. The plan's stated rationale — "later
  edits to the agent do not silently change which provider the session resumes
  with" — applies equally to the model, and silently changing models across a
  resume is worse than changing endpoints.
- `history-limit` must be set before the pane is created; it belongs in the
  locked tmux config file, not in a post-attach `set-option`.
- `COPILOT_PROVIDER_BEARER_TOKEN` takes precedence over the API key. Ensure it
  cannot be inherited from a stray environment and silently override the
  configured connection.
- `POST /terminal/ensure` is a non-idempotent verb doing an idempotent thing.
  Document the idempotency key (the session ID) and concurrent-call semantics in
  the API contract itself, not only in prose.
- The acceptance matrix is almost entirely manual, which means it runs once.
  Commit to a fake in-process terminal runtime for unit tests plus a small
  Docker-gated integration suite, and mark which of the ten items are
  automated.
- Docs to update at completion: `docs/OVERVIEW.md`, `docs/PLAYWRIGHT.md` for the
  new fixtures, and a new `docs/TERMINAL_PROTOCOL.md` for the frame protocol,
  which the plan promises to document but does not assign a home.

### On the PM note about multi-browser support

The PM's note offers to drop shared multi-browser attachment if it costs
robustness. Worth flagging that the trade is not available in the shape it
implies: tmux is what buys *disconnect survival*, and multi-client attach is a
free consequence of tmux rather than an extra mechanism layered on top. The
real cost of multi-attach is confined to phantom-client reconciliation
(item 10) and the mixed-size policy (already in the risk table, one-line fix by
switching to `window-size smallest`).

So the escape hatch should be scoped as "enforce a single active attachment in
`client_service`" — a policy check, cheap to add and cheap to remove — rather
than "remove the multiplexer", which would sacrifice the plan's primary
requirement. Recommend keeping multi-attach and noting the policy-level
fallback.

### Phasing

Phase ordering is good. One suggestion: Phase 2 is both independently
shippable and the only phase that changes existing Chat behavior. Land and soak
it separately from Phase 3 so that any Chat regression in stable IDs, adoption,
or non-destructive shutdown is not entangled with terminal work during
diagnosis.

### What the plan gets right

Recorded so it is not lost in revision: tmux owning the PTY rather than a
WebSocket or an `agent_host` task; stable client-side session IDs with label
based adoption and idempotent ensure; a separate `CliHarnessName` enum instead
of implying every kernel harness has a CLI; a nested optional `cli` block
instead of three independently invalid flat fields; refusing to model PTY bytes
as a `Kernel` protocol method; binary frames with bounded per-attachment
queues; reusing existing Connection and secret entities instead of duplicating
credentials; distinguishing reattach from resume in the UI; and an unusually
honest non-goals list. The failure-mode coverage is well above average for a
plan at this stage.

## Response to Feedback

The review was checked against the current repository, the installed Copilot
CLI help, and the relevant upstream behavior. Most of its corrections are
accepted. Where this response conflicts with the original plan, this section
describes the implementation adjustment; the original plan and review remain
unchanged as requested.

### Accepted architectural adjustments

1. **State the terminal trust boundary accurately.** CLI View is command
   execution inside the selected session container. It is not a security
   boundary against the user operating that session, because Copilot itself can
   run commands even if tmux cannot create another shell. The implementation
   will still use a dedicated locked tmux configuration, no usable prefix, and
   unbound root/prefix command keys to prevent accidental pane/window creation
   and layout damage. The original "not a general remote shell" wording should
   be read as "not an arbitrary host-shell launcher," not as a claim that the
   attached user lacks container command execution.

2. **Move Copilot launch construction into `kernel_host`.** The review is
   correct that a Rust `CopilotCliLauncher` in `agent_host` cannot share launch
   logic with the Python `kernel_copilot` implementation. `kernel_host` will
   own a Copilot terminal controller and internal ensure/status/stop endpoints.
   Copilot argv construction, provider mapping, model/effort handling, paths,
   custom-agent preparation, and session-ID semantics will live in one Python
   module shared by Chat's Copilot kernel and the terminal controller.
   `agent_host` remains harness-agnostic and owns container adoption, Docker
   exec attach, byte forwarding, resize, and terminal resource lifecycle. This
   remains outside the `Kernel` event protocol.

3. **Use tmux as the cross-process idempotency primitive.** Terminal ensure
   will unconditionally attempt the named tmux session creation and interpret
   the atomic "already exists" result as adoption. A per-session async lock may
   remain to avoid duplicate work inside one service process, but it is not the
   correctness mechanism.

4. **Use `--session-id=<uuid>` for both first launch and recovery.** The
   installed Copilot CLI explicitly documents this flag as resuming an existing
   session or assigning the UUID for a new one. Using it unconditionally removes
   the crash-sensitive "have we launched before?" branch while preserving the
   required durable Copilot resume behavior. A Docker-gated test will exercise
   both absent-session and existing-session cases against the supported Copilot
   CLI version. If that upstream contract changes, the launcher must detect the
   capability and fail explicitly rather than guess from persisted launch
   state.

5. **Observe pane liveness.** A tmux session is not proof that Copilot is
   running when `remain-on-exit` is enabled. Terminal status will inspect
   `pane_dead`, `pane_dead_status`, and `pane_pid` and return
   `running`, `exited(status)`, or `missing`. Ensure and the WebSocket `ready`
   message will also return `attach_kind: started | attached | resumed`, so the
   UI can distinguish exact live reattachment from process recovery.

6. **Make tmux the sole scrollback owner.** xterm will use `scrollback: 0`;
   the server will not replay `capture-pane` output before attach. An explicit
   UI action will ask the terminal controller to enter tmux copy mode, and the
   UI will explain how to leave it. This avoids duplicate history and
   alternate-screen redraw corruption. The tmux history limit will be set in
   the locked configuration before pane creation.

7. **Give application mouse handling to Copilot.** The locked tmux
   configuration will set `mouse off`, while Copilot runs with `--mouse=on`.
   Scrollback is therefore entered explicitly rather than by having tmux steal
   wheel events from Copilot's alternate screen.

8. **Make Unicode behavior an explicit compatibility target.** The kernel
   image will set `LANG=C.UTF-8` and `LC_ALL=C.UTF-8`, tmux will be started in
   UTF-8 mode where supported, and xterm's Unicode width implementation will be
   selected deliberately. Automated browser/integration coverage will use a
   fixed sample containing ASCII, box drawing, combining marks, CJK, emoji, and
   a multi-code-point emoji sequence, checking cursor placement after resize
   and reattach.

9. **Reconcile terminal attachments.** Each WebSocket attachment will have an
   AgentSpace attachment ID and a deterministically discoverable tmux client.
   Normal close detaches that client. Adoption after an `agent_host` restart
   detaches clients left by connections that cannot still exist, and periodic
   reconciliation compares known live attachments with `tmux list-clients`.
   Terminal summaries source `attachment_count` from observed tmux clients, not
   an in-memory counter. The implementation must prove this mapping in
   integration tests before simultaneous attachment is considered complete.

10. **Define concrete non-destructive shutdown cleanup.** Phase 2 will include
    an explicit reconciliation/cleanup API and `stack-down` changes that remove
    running and exited AgentSpace kernel containers plus managed
    session-workspace volumes with no owning durable client session. Ordinary
    service restart remains non-destructive. Cleanup will support a dry-run or
    report mode so ownership decisions are inspectable.

11. **Label every managed runtime resource with the full durable identity.**
    Kernel containers and session-workspace volumes will carry the full
    `agentspace.session_id`, interaction mode, and managed-resource labels.
    Truncated names are cosmetic only and cannot be used for adoption,
    collision checks, or garbage collection.

12. **Represent legacy sessions explicitly.** Pre-migration sessions without
    stable labels remain usable while their original `agent_host` registry and
    container are alive, but they are not promised adoption after that runtime
    is lost. The API/UI will expose a `legacy-unrecoverable` (or equivalently
    typed) recovery state instead of turning this into an unexplained generic
    failure. No risky background rewrite of already-running legacy containers
    is planned.

13. **Use Copilot's secret-environment protection as defense in depth.** The
    Copilot command will mark provider credentials and any retained generic
    connection credential variables with `--secret-env-vars`. Known provider
    credential variables, especially `COPILOT_PROVIDER_BEARER_TOKEN`, will be
    cleared before the selected Connection is applied so inherited state cannot
    override it. Unneeded generic credential duplicates will not be copied into
    the Copilot child. This reduces accidental shell/MCP/output exposure, but
    the documentation will state plainly that a root-capable session container
    is not a secrecy boundary against its own operator.

14. **Place AgentSpace-generated Copilot files in the session workspace.** The
    generated custom-agent profile will live under `.github/agents`, alongside
    the session-scoped `.github/skills` projection, rather than in the shared
    Copilot home. File names will be AgentSpace-owned and collision-resistant.
    Workspace snapshotting currently supports only top-level-name exclusions,
    so it must gain safe relative-path exclusions (or equivalent staging)
    before this ships. Saving a workspace will omit only AgentSpace-generated
    profiles and skill links while preserving user-authored `.github` content.

15. **Fix Copilot skill and connection parity for Chat too.** The shared
    `/root/.copilot/skills` race is pre-existing and affects concurrent Chat
    containers, not just CLI. The session-scoped skill projection and shared
    Python Copilot launch/provider builder will apply to both surfaces.
    Consequently, Copilot Chat will also translate the existing Connection into
    `COPILOT_PROVIDER_*`; `chat_completions` will explicitly set
    `COPILOT_PROVIDER_WIRE_API=completions` even though that is Copilot's
    current default. Shipping CLI-only BYOK semantics would violate the intended
    parity and is not acceptable.

### Data, protocol, testing, and documentation adjustments

- Terminal input and output are **raw bytes** in both directions. Browser
  adapters encode xterm's text input and preserve its binary-input callback
  without imposing protocol-level UTF-8 validity.
- Upgrade rejection happens before a WebSocket exists, so origin rejection and
  any future authentication rejection use HTTP status responses, not WebSocket
  close codes. After upgrade, the terminal protocol will reserve documented
  private close codes for session-gone, terminal-state conflict, slow-client
  queue overflow, and upstream loss, while retaining standard close codes for
  normal detach and internal failure.
- A durable CLI launch snapshot will include the non-secret provider shape,
  selected model, reasoning effort, relevant Copilot options, and references to
  the Connection/secret inputs. Secret values are resolved at launch. This
  avoids silently changing model or wire behavior during resume while still
  permitting credential rotation.
- `POST /sessions/{id}/terminal/ensure` will document the session ID as its
  idempotency key and specify concurrent-call and retry semantics in the API
  contract.
- The terminal runtime test double will be an in-process fake used by service
  contract tests. A small opt-in Docker integration suite will cover tmux
  creation/adoption, PTY resize, simultaneous attach, service restart,
  process/container loss, Unicode width, and durable Copilot session recovery.
  The acceptance matrix will identify which checks are automated; visual
  rendering and true cross-device behavior may remain manual smoke checks.
- Completion documentation will update `docs/OVERVIEW.md` and
  `docs/PLAYWRIGHT.md` and add `docs/TERMINAL_PROTOCOL.md` for framing, upgrade
  errors, close codes, limits, and reconnect behavior.
- Phase 2 will land and soak separately because it changes stable identity,
  shutdown, adoption, and cleanup for existing Chat sessions before terminal
  execution is introduced.
- Model/effort/provider snapshot migrations and full-label adoption tests move
  into Phase 1/2 rather than being deferred to hardening.

### Feedback not adopted

1. **No attach-ticket mechanism in v1.** The review is correct that Origin
   validation is not authentication. However, the current product is explicitly
   a trusted, single-user local system with no authentication anywhere. A
   single-use ticket issued by an unauthenticated HTTP endpoint would not
   identify or authorize a principal; it would add state and URL-handling
   complexity without creating the missing trust boundary. V1 will keep the
   service bound to the configured local/trusted interface, validate browser
   origins as CSRF defense, avoid exposing container endpoints, and document
   that the API must not be placed on an untrusted network. A future real
   authentication design should use the same authenticated session as the rest
   of `client_service`; a short-lived attach ticket may then be useful for the
   browser WebSocket handshake. No certificates or local PKI are required.

2. **No hard pin of the distribution tmux package in v1.** Exact apt package
   pins against a moving Debian base are brittle and do not by themselves
   guarantee terminal-width compatibility. The image will instead assert a
   supported minimum tmux version during build/startup, log the effective
   version, keep the locked configuration under source control, and gate the
   required behavior with integration tests. The container image/lock strategy
   can pin a full package snapshot later if reproducible base images become a
   project-wide requirement.

3. **No automatic age/idle reaper in v1.** Time-based deletion conflicts with
   the requirement that disconnected sessions remain resumable and risks
   destroying valid work. Explicit deletion, startup reconciliation, orphan
   reporting, the cleanup API, and corrected `stack-down` cleanup are required.
   An opt-in retention policy can be designed later with a durable last-active
   definition.

4. **No planned single-attachment fallback.** Simultaneous interactive
   attachment was explicitly selected as a requirement. Tmux remains necessary
   for disconnect survival regardless, and attachment reconciliation must be
   made robust rather than silently reducing the feature. A future deployment
   policy could enforce one attachment without changing the runtime, but v1
   will not claim completion until the multi-browser tests pass.

5. **No WebSocket close code for a rejected upgrade.** As noted above, an
   origin/auth failure rejected during the HTTP handshake cannot also send a
   WebSocket close frame. The protocol will document the HTTP response and the
   client behavior separately from post-upgrade close codes.
