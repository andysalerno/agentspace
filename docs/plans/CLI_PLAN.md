# CLI View Implementation Plan

## Status

Final implementation plan.

## Goal

Add a first-class **CLI** view to the web UI that runs the interactive GitHub
Copilot CLI inside an AgentSpace session container and renders its terminal in
the browser.

The first release must:

- use existing AgentSpace agents, connections, secrets, skills, workspaces, and
  sessions;
- expose only agents explicitly configured for CLI use;
- support Copilot CLI even where Copilot-specific behavior is required;
- provide ANSI colors, Unicode and emoji, alternate-screen applications, mouse
  input, paste, resize, and terminal scrollback;
- allow multiple browsers to interact with the same live terminal;
- preserve the exact live PTY and Copilot process across browser disconnects
  and AgentSpace service restarts while the session container remains alive;
- recover a lost process, container, or host using Copilot's durable session ID
  and the persistent session workspace;
- provide the same **VS Code** action as Chat;
- keep clients behind `client_service`; and
- expose a client-neutral terminal API suitable for a future AgentSpace CLI
  client without implementing that client now.

## Trust Model

AgentSpace is currently a trusted, single-user local system without
general-purpose authentication. CLI View grants command execution inside the
selected session container. Copilot can run commands by design, so the terminal
is not a security boundary against the user operating that session.

The implementation will still prevent accidental expansion of the terminal
surface:

- only an allowlisted configured CLI may be launched;
- no arbitrary command template is accepted from configuration or the browser;
- tmux uses a locked AgentSpace configuration with no usable prefix and no
  root/prefix commands for creating shells, windows, or panes;
- the browser never receives Docker or container credentials;
- container endpoints are not exposed directly to the browser;
- secrets are resolved only when required and are redacted from logs; and
- Copilot's secret-environment filtering is enabled as defense in depth.

The API must remain bound to a local or otherwise trusted interface. Origin
validation is a browser CSRF control, not authentication. V1 will not add local
certificates, a PKI, or a single-use attach ticket issued by an otherwise
unauthenticated service. A future product-wide authentication design may add
short-lived WebSocket attach tickets after an authenticated HTTP request.

## Non-Goals

- Supporting interactive CLIs other than Copilot CLI in v1.
- Implementing CLI View in `clients/cli_ui` or `channels/cli_channel`.
- Converting terminal output into Chat messages or storing it in
  `client_messages`.
- Preserving a live process or exact terminal screen across a host reboot.
- Allowing Chat and CLI to drive the same Copilot session concurrently.
- Converting a session between incompatible harnesses.
- Exposing arbitrary host or container shell launch definitions.
- Adding an automatic age- or idle-based session reaper.
- Providing authentication only for terminal routes while the rest of
  `client_service` remains unauthenticated.

## Current State and Gaps

The existing service boundary remains authoritative:

```text
Web UI -> client_service -> agent_host -> per-session kernel container
```

Current behavior:

- `client_service` owns the public API, durable SQLite session records, agents,
  connections, secrets, configuration validation, and workspace metadata.
- `agent_host` owns Docker/container lifecycle but keeps its runtime session
  registry only in memory.
- Each kernel container receives resolved agent environment, skills, workspace
  mounts, a persistent `/workspace` volume, the shared Copilot state volume at
  `/root/.copilot`, and code-server support used by the VS Code action.
- Chat invokes Copilot in non-interactive prompt mode through `kernel_host`.
- Copilot exposes a resume token, but `client_service.SessionRecord` does not
  persist it.
- `agent_host` generates a separate runtime session ID and currently destroys
  every registered session during graceful shutdown.
- Container and workspace-volume names use truncated runtime IDs; their labels
  do not carry a durable client session identity.
- There is no PTY abstraction, Docker exec/resize support, WebSocket terminal
  transport, or xterm component.
- Nginx proxies streaming HTTP but does not forward WebSocket upgrades.
- Copilot Chat receives generic `CONNECTION_*` values but does not translate
  them to Copilot's `COPILOT_PROVIDER_*` BYOK variables.
- Copilot skills are projected into the shared `/root/.copilot/skills`
  directory, so concurrent Chat containers can alter one another's enabled
  skill set.
- Workspace snapshots can exclude only top-level names, not individual
  AgentSpace-owned files under `.github`.

CLI View therefore adds an opaque terminal execution path alongside the
normalized kernel event path. PTY bytes do not belong in the `Kernel` event
protocol.

## Configuration Model

### Agent CLI capability

Add an optional nested CLI capability to an agent. Presence of the block means
the agent is available in CLI View.

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

Proposed model:

```rust
struct AgentCliConfig {
    harness: CliHarnessName,
    connection: Option<String>,
}

enum CliHarnessName {
    CopilotCli,
}
```

`CliHarnessName` is intentionally separate from `HarnessName`. A kernel harness
is not automatically an interactive CLI. Supporting another CLI requires an
explicit launcher/controller implementation and enum addition.

`cli.connection` references the existing Connection entity. It does not
duplicate a URL or credential. When omitted, Copilot uses its normal GitHub
authentication from the persistent Copilot state volume. When CLI is enabled
in the UI, its connection picker initially uses the agent's Chat connection,
but the two references may differ.

Required configuration work:

- add `cli` to `config/document.rs`, `AgentRecord`, config adapters, config-set
  loading, canonical YAML, resource export, bundle handling, CRUD requests, API
  responses, and web types;
- validate the CLI connection reference with an exact field path;
- preserve `deny_unknown_fields` and strict canonical round trips;
- reject unsupported CLI harnesses; and
- show CLI eligibility, harness, and connection in Agents View.

### Shared Copilot parity

The same Copilot launch/provider module must be used by:

- non-interactive Copilot Chat; and
- interactive Copilot CLI.

This shared module owns model, effort, provider translation, additional paths,
secret filtering, session-ID flags, custom-agent profile creation, and
session-scoped skill projection. CLI View must not introduce provider or skill
semantics that disagree with Copilot Chat.

## Durable Session Model

### Interaction mode

`client_type` identifies the caller and must not be overloaded to mean Chat or
CLI. Add an explicit interaction mode:

```text
interaction_mode: chat | cli
```

`POST /sessions` defaults this field to `chat` for backward compatibility. In
v1:

- Chat message endpoints reject CLI sessions with `409 Conflict`;
- terminal endpoints reject Chat sessions with `409 Conflict`;
- Chat and CLI views filter the shared session list by mode; and
- deletion, workspace save, runtime inspection, and VS Code remain shared.

The mode records the active session surface rather than permanently splitting
the session model. A later transition endpoint may stop one Copilot surface and
resume the same durable harness session in the other. Concurrent Chat and CLI
drivers are not allowed.

### CLI session fields

Persist at least:

```text
session_id
agent_id
interaction_mode
cli_harness
cli_connection_id
harness_session_id
runtime_generation
runtime_status
workspace_volume_identity
created_at
updated_at
```

`harness_session_id` is a generated Copilot UUID known before the first launch.
It is independent of the currently live container or tmux process.

Persist a normalized, non-secret launch snapshot containing:

- provider type and wire API;
- literal non-secret provider values or references to config/secret values;
- selected model and reasoning effort;
- relevant Copilot options;
- additional path identities; and
- the agent custom-profile identity.

Resolved secret values are never stored in the session row. They are resolved
at launch/recovery time, permitting credential rotation without silently
changing model, provider type, wire API, or endpoint selection.

Use SQLite `ensure_column` migrations and explicit parsing defaults so existing
records become `interaction_mode=chat`.

### Legacy sessions

Pre-migration sessions without stable runtime labels remain usable while their
original `agent_host` registry and container remain alive. They are not
promised adoption after that runtime is lost.

Expose this as a typed `legacy-unrecoverable` recovery state rather than a
generic failure. Do not rewrite labels on already-running legacy containers in
the background.

## Stable Runtime Identity

Generate the durable `client_service` session ID before contacting
`agent_host`, and pass it as the requested runtime identity.

New runtime creation is idempotent:

- if the session is registered, return it;
- if a correctly labeled container is running, adopt it;
- if the container is missing or stopped, recreate it using the same durable
  session and workspace identity; and
- if a name or label points to another identity, fail rather than attach.

Use the full session ID in labels:

```text
agentspace.role=kernel
agentspace.managed=true
agentspace.session_id=<full client session id>
agentspace.interaction_mode=<chat|cli>
```

Session-workspace volumes carry the same full `agentspace.session_id` and
managed-resource labels. Truncated names are cosmetic only and cannot be used
for adoption, collision checks, ownership, or garbage collection.

`client_service` writes a recoverable `starting` session row before or
transactionally around upstream creation. A failed launch becomes an explicit
`error` state that can be retried or deleted.

## Component Responsibilities

### `client_service`

`client_service` remains the only public backend.

It owns:

- durable session/configuration state;
- CLI eligibility and mode validation;
- connection and secret resolution;
- public terminal status/control routes;
- the browser-facing WebSocket;
- the upstream WebSocket proxy;
- origin validation;
- public error/status shaping; and
- filtering secrets and internal container details from responses.

### `agent_host`

`agent_host` remains harness-agnostic.

It owns:

- stable container creation and adoption;
- session workspace volume creation/reuse/deletion;
- Docker exec with TTY/stdin/stdout/stderr;
- Docker exec resize;
- byte forwarding and per-attachment backpressure;
- attachment lifecycle and reconciliation;
- calling the internal `kernel_host` terminal controller; and
- runtime, VS Code, port, and container summaries.

It does not construct Copilot argv or translate provider settings.

### `kernel_host`

`kernel_host` owns harness-specific terminal control inside the session
container.

It exposes internal-only endpoints:

```text
POST /terminal/ensure
GET  /terminal
POST /terminal/stop
POST /terminal/resume
POST /terminal/copy-mode
```

These endpoints:

- prepare AgentSpace-owned custom-agent and skill files;
- construct Copilot argv/environment through shared Python code;
- create or adopt the tmux session atomically;
- observe pane status;
- respawn an exited Copilot pane with the same durable session ID;
- enter copy mode;
- stop the pane/session; and
- return structured terminal state.

They do not carry PTY bytes. Attach remains a Docker exec managed by
`agent_host`.

### Web UI

The web UI owns:

- xterm rendering;
- user input encoding;
- terminal fit/resize observation;
- reconnection UX;
- status/error presentation;
- session selection and creation;
- theme and renderer fallback; and
- shared workspace, delete, and VS Code actions.

## Copilot Launch Semantics

### Session identity

Generate one UUID for the Copilot session and persist it before launch. Always
start Copilot with:

```text
--session-id=<uuid>
```

The supported Copilot CLI documents this flag as either assigning the UUID for
a new session or resuming an existing session. Using it unconditionally avoids
a crash-sensitive persisted "launched before" state machine.

A Docker-gated compatibility test must exercise both:

- no existing Copilot session for the UUID; and
- an existing Copilot session for the UUID.

If a future Copilot version changes this contract, launch must fail explicitly
or use a capability-driven fallback. It must not infer first launch from stale
AgentSpace state.

### Arguments

The shared Copilot builder supplies:

- interactive mode for CLI View or prompt mode for Chat;
- `--session-id=<uuid>`;
- `--no-auto-update`;
- `--mouse=on` for interactive CLI;
- configured model and reasoning effort;
- `--agent=<session profile>` when a system prompt is configured;
- `--add-dir` entries for allowed additional paths;
- `/workspace` as the working directory;
- validated Copilot extra arguments as individual argv entries; and
- `--secret-env-vars` for provider/connection credential names.

No shell command string is built. Every argument remains a separate argv entry.

### Provider mapping

The selected Connection maps to Copilot BYOK variables:

| AgentSpace value | Copilot variable |
| --- | --- |
| connection URL | `COPILOT_PROVIDER_BASE_URL` |
| connection API key | `COPILOT_PROVIDER_API_KEY` |
| `chat_completions` | `COPILOT_PROVIDER_WIRE_API=completions` |
| `responses` | `COPILOT_PROVIDER_WIRE_API=responses` |
| configured model | `COPILOT_MODEL` |

The current Connection model represents OpenAI-compatible endpoints, so v1
sets `COPILOT_PROVIDER_TYPE=openai`. OpenRouter is supported through its
OpenAI-compatible API. Azure and Anthropic wait for an explicit provider type
in the shared Connection model; URL guessing is not allowed.

Before applying the selected Connection, clear known inherited Copilot provider
variables, including `COPILOT_PROVIDER_BEARER_TOKEN`, so stray environment
cannot override the configured provider. Do not forward unnecessary generic
credential duplicates to the Copilot child.

`--secret-env-vars` reduces accidental exposure to shell/MCP children and
redacts output, but the container remains trusted to its operator.

### System prompt and skills

Copilot has no generic `--system-prompt` option. Materialize the AgentSpace
system prompt as a valid, deterministic custom-agent profile under:

```text
/workspace/.github/agents/
```

Select it with `--agent`. Use an AgentSpace-owned, collision-resistant file
name derived from the durable session identity. An empty prompt removes only
the owned stale profile and omits `--agent`.

Project enabled skills into:

```text
/workspace/.github/skills/
```

Use AgentSpace-owned links to the read-only skill staging mount. Reconciliation
replaces only owned links and preserves unrelated user-authored files.

Apply this session-scoped projection to both Copilot Chat and CLI. Do not write
per-session enabled skills into shared `/root/.copilot/skills`.

Workspace save must exclude only AgentSpace-generated profiles and skill links.
Extend snapshotting to support validated relative-path exclusions or an
equivalent safe staging mechanism; preserve all user-authored `.github`
content.

## Live Terminal Runtime

### Tmux ownership

Install tmux in the kernel image and run one private tmux session per
AgentSpace CLI session.

Tmux, not the WebSocket or an `agent_host` task, owns the live PTY. This makes
the process independent of browser and service connections.

Use a source-controlled locked tmux configuration that:

- disables the prefix;
- unbinds root/prefix commands that can create shells, windows, or panes;
- sets `remain-on-exit on`;
- sets `destroy-unattached off`;
- sets `mouse off`;
- sets a finite, generous `history-limit` before pane creation;
- selects `window-size smallest` for mixed-size clients; and
- contains no user-controlled command fragments.

Do not hard-pin an exact Debian tmux package version in v1. Assert a supported
minimum version during image build/startup, log the effective version, and test
the required behavior in the Docker integration suite.

Set:

```text
LANG=C.UTF-8
LC_ALL=C.UTF-8
```

Start tmux in UTF-8 mode where supported.

### Atomic ensure

`kernel_host` unconditionally attempts named tmux session creation. Tmux's
duplicate-session result is the cross-process correctness primitive:

- creation succeeded: the terminal was started or resumed from durable Copilot
  state;
- session exists and pane is running: attach to the exact live terminal;
- session exists and pane is dead: report `exited(status)` and require an
  explicit resume action; and
- tmux session is missing: create it using the durable launch snapshot.

An in-process lock may suppress redundant work but cannot be required for
correctness.

### Observed status

Inspect tmux pane formats including:

```text
pane_dead
pane_dead_status
pane_pid
```

Return:

```text
missing
running
exited(status)
```

Ensure also returns:

```text
attach_kind: started | attached | resumed
```

The UI uses `attach_kind` to distinguish exact live reattachment from a new
process that resumed durable Copilot state.

### Attach

Each browser attachment creates a Docker exec with TTY, stdin, stdout, and
stderr attached, running only the fixed AgentSpace tmux attach argv.

Each attachment has:

- an AgentSpace attachment ID;
- a deterministically discoverable tmux client;
- a Docker exec/resize handle;
- bounded inbound and outbound queues; and
- explicit detach cleanup.

Multiple attachments may type concurrently. `window-size smallest` ensures the
shared pane fits every attached client; larger clients may show padding.

### Attachment reconciliation

Normal WebSocket close detaches its tmux client. Reconciliation compares active
AgentSpace attachments with `tmux list-clients`:

- detach stale clients left by lost proxy/exec connections;
- on `agent_host` adoption after restart, detach clients whose previous service
  connections cannot still exist;
- source `attachment_count` from observed tmux clients; and
- reap completed Docker exec attachments where supported.

This mapping must be proven by integration tests. Simultaneous interactive
attachment remains a v1 requirement; a silent single-attachment fallback is
not acceptable.

### Scrollback and mouse

Tmux is the sole scrollback owner:

- configure xterm with `scrollback: 0`;
- do not replay `capture-pane` before attach;
- expose a UI action that invokes `POST /terminal/copy-mode`; and
- explain the copy-mode exit key in the UI.

Tmux mouse handling remains off so Copilot receives alternate-screen mouse
events. Copilot runs with `--mouse=on`.

## Public Terminal API

### HTTP routes

```text
POST /sessions
  { agent_id, interaction_mode: "cli", ... }

GET /sessions/{session_id}
  includes interaction mode, terminal summary, recovery state, and runtime URLs

POST /sessions/{session_id}/terminal/ensure
  starts, adopts, or resumes the terminal

GET /sessions/{session_id}/terminal
  returns current terminal status

GET /sessions/{session_id}/terminal/ws
  upgrades to a terminal WebSocket

POST /sessions/{session_id}/terminal/stop
  stops live tmux/Copilot while preserving durable session/workspace state

POST /sessions/{session_id}/terminal/resume
  respawns an exited pane with the same durable Copilot session ID

POST /sessions/{session_id}/terminal/copy-mode
  { attachment_id }
  enters tmux copy mode for the selected attachment
```

The durable session ID is the idempotency key for `terminal/ensure`.
Concurrent calls must produce at most one tmux session and one Copilot pane.
Retries return the observed existing result.

Opening a CLI session calls `ensure`, then attaches. Browser reload and
concurrent clients do not launch duplicate Copilot processes. `resume` is valid
only for an observed exited pane; concurrent resume requests use tmux's atomic
pane/session state and return the same resulting process state.

### WebSocket framing

Use a documented protocol independent of xterm's attach addon.

Client to server:

- binary frame: raw terminal input bytes;
- text frame:
  `{"type":"resize","cols":120,"rows":40}`.

Server to client:

- binary frame: raw PTY output bytes;
- text `ready` frame with terminal status, `attach_kind`, dimensions, and
  attachment ID;
- text lifecycle frames for `exited` and `error`.

WebSocket close means detach only. It never implies stop.

Validate bounded positive rows and columns before Docker resize. Use bounded
queues in both directions. A slow client is detached explicitly without
stopping tmux or other clients.

### Upgrade errors and close codes

Reject invalid upgrades with HTTP responses before a WebSocket exists:

| Condition | HTTP status |
| --- | --- |
| invalid browser Origin | `403 Forbidden` |
| session not found | `404 Not Found` |
| wrong interaction mode/state | `409 Conflict` |
| terminal upstream unavailable | `503 Service Unavailable` |

After upgrade, reserve:

| Code | Meaning |
| --- | --- |
| `1000` | normal detach |
| `1011` | unexpected internal failure |
| `4404` | session disappeared |
| `4409` | terminal state conflict |
| `4429` | client detached for backpressure/queue overflow |
| `4503` | terminal upstream unavailable |

Document reconnect behavior for every status/code in
`docs/TERMINAL_PROTOCOL.md`.

### Proxying and origin handling

- Enable Axum's WebSocket support in both Rust services.
- Add a WebSocket client dependency to `client_service` for the upstream
  `agent_host` connection.
- Validate browser `Origin` against the existing configured allowed origins.
- Continue supporting non-browser/future CLI clients without inventing browser
  Origin headers.
- Update Nginx to forward `Upgrade` and `Connection` headers and use a
  terminal-appropriate idle timeout.
- Build the browser WebSocket URL from the current origin plus `/api`; never
  embed container or host ports.
- Reconnect with bounded exponential backoff only for retryable detach causes.

## Persistence and Recovery

### Level 1: exact live terminal

The live hierarchy is:

```text
kernel container
  -> tmux server/session
    -> Copilot CLI process
```

While that hierarchy is alive:

- closing a tab or browser changes nothing;
- another device can attach to the exact terminal;
- multiple browsers can interact concurrently;
- restarting the web UI or `client_service` changes nothing;
- restarting `agent_host` does not terminate the container/tmux/Copilot; and
- the new `agent_host` adopts the labeled container and reconciles attachments.

`agent_host` must not destroy kernel sessions merely because it receives a
normal shutdown signal. Explicit deletion and explicit stack cleanup remain
destructive.

### Level 2: durable Copilot recovery

The shared Copilot state volume contains durable Copilot session data. The
stable session-workspace volume contains working files and AgentSpace's
session-scoped custom-agent/skill projection.

After process, container, or host loss:

1. CLI View loads the durable session from `client_service`.
2. `terminal/ensure` observes that no live pane/container exists.
3. `agent_host` recreates the container with the same stable workspace volume.
4. Current secret values are resolved against the durable non-secret launch
   snapshot.
5. `kernel_host` launches Copilot with the same `--session-id=<uuid>`.
6. Copilot resumes if that UUID exists or creates it if the previous launch
   never committed session state.
7. The UI reports `attach_kind=resumed` and attaches to the new tmux session.

If required configuration, secret data, Copilot state, or the workspace volume
is missing, fail with a specific recovery error. Never silently replace an old
session with a new UUID.

## Cleanup and Reconciliation

Durable `client_service` sessions are authoritative for user-facing ownership.

Provide an explicit cleanup/reconciliation operation with report/dry-run mode
that can:

- list labeled containers and session-workspace volumes;
- adopt resources owned by a durable session;
- identify running or exited orphan containers;
- identify managed session volumes with no durable owner;
- remove explicitly selected orphan resources; and
- report legacy resources that cannot be safely matched.

Update `just stack-down` to remove running and exited AgentSpace kernel/gateway
containers and their managed orphaned session resources. It must not rely on
`docker ps`'s running-only default.

Do not add an automatic age/idle reaper in v1. Disconnected sessions are
intentionally persistent, and time alone is not evidence that data may be
destroyed.

## Web UI

### Navigation and layout

- Add `cli` to `ViewId` and place **CLI** directly after **Chat**.
- Implement `CliView.tsx` with:
  - a left rail of CLI sessions;
  - New Session;
  - a main terminal pane; and
  - a header with status and actions.
- Filter the agent picker to `agent.cli != null`.
- Keep selected Chat and CLI session IDs separately in `App`.
- Reuse existing delete, save-workspace, browser URL, status, and confirmation
  behavior where semantics match.

### Terminal component

Use xterm.js, the terminal component used by VS Code:

```text
@xterm/xterm
@xterm/addon-fit
@xterm/addon-unicode11
@xterm/addon-webgl
```

Requirements:

- use `FitAddon` with `ResizeObserver`;
- send dimensions after attach and on actual size changes;
- use a deliberate Unicode width implementation;
- attempt `WebglAddon` and fall back on unsupported devices/context loss;
- use `scrollback: 0` because tmux owns history;
- pass xterm input as raw binary WebSocket frames;
- pass PTY output bytes directly to `terminal.write`;
- preserve xterm mouse reporting for Copilot;
- derive terminal colors from AgentSpace's active theme;
- use a monospace stack with color-emoji fallback;
- dispose terminal, addons, listeners, observer, reconnect timer, and socket on
  session change/unmount;
- focus after successful attachment without stealing focus from dialogs; and
- provide accessible status/error text outside the canvas.

Do not use `@xterm/addon-attach`; the custom protocol includes lifecycle,
resize, errors, attachment identity, and backpressure semantics that the generic
addon does not model.

### Header and recovery UX

Show:

- agent name;
- terminal state (`starting`, `live`, `exited`, `disconnected`, `resuming`,
  `error`, or legacy recovery limitation);
- whether the last ensure started, attached, or resumed;
- compact AgentSpace session ID;
- Copilot session ID in details;
- observed attachment count;
- **VS Code**, using the same runtime URL and
  `browserReachableLocalUrl` behavior as Chat;
- enter scrollback/copy mode;
- reconnect/resume when applicable;
- save workspace;
- stop CLI; and
- delete session.

Navigating away or closing the browser only detaches. **Stop CLI** terminates
the live terminal but preserves durable recovery data. **Delete session**
remains destructive and follows the existing save-workspace prompt.

### Unicode acceptance sample

Automated rendering/PTY tests must include a fixed sample covering:

- ASCII;
- box-drawing characters;
- combining marks;
- full-width CJK;
- a single-code-point emoji; and
- a multi-code-point emoji sequence.

Verify cursor placement before/after resize and after live reattachment.

## Implementation Sequence

### Phase 1: configuration and durable model

1. Add `CliHarnessName` and `AgentCliConfig`.
2. Wire CLI configuration through strict YAML, adapters, CRUD, export, bundles,
   validation, web types, and Agents View.
3. Add interaction mode, CLI session fields, launch snapshot, resume identity,
   recovery states, and SQLite migrations.
4. Extend session create/list/get contracts while preserving Chat defaults.

Exit criterion: CLI configuration round-trips strictly, and a durable CLI
session can be created in an explicit `starting` state.

### Phase 2: stable identity, adoption, and cleanup

1. Let `client_service` allocate and pass the stable runtime ID.
2. Add full labels to containers and session-workspace volumes.
3. Make runtime ensure/create idempotent and add safe container adoption.
4. Reuse stable workspace volumes on recovery.
5. Remove destructive cleanup from ordinary `agent_host` shutdown.
6. Add explicit orphan reporting/cleanup and correct `stack-down`.
7. Represent pre-migration sessions as legacy when adoption is unavailable.

Exit criterion: Chat sessions created under the new model survive Rust service
restarts, cleanup has no unbounded disk-growth regression, and all existing
Chat tests pass. Land and soak this phase separately before terminal work.

### Phase 3: shared Copilot launch parity

1. Extract one Python Copilot argv/environment/provider builder.
2. Add explicit BYOK translation for Copilot Chat and future CLI.
3. Move Copilot skill projection to session-scoped `.github/skills`.
4. Add session-scoped custom-agent profile generation.
5. Add validated relative-path snapshot exclusions.
6. Add unconditional `--session-id` and secret-environment handling.

Exit criterion: Copilot Chat and the forthcoming terminal controller share
tested provider, model, prompt, path, skill, and session semantics.

### Phase 4: kernel terminal controller and tmux

1. Add tmux, locale settings, minimum-version validation, and locked config.
2. Implement internal ensure/status/stop/copy-mode endpoints.
3. Implement atomic creation, observed pane status, and attach-kind results.
4. Test first launch, exact live adoption, dead pane, stop, and durable resume.

Exit criterion: `kernel_host` can manage one persistent Copilot tmux session
without carrying PTY bytes over HTTP.

### Phase 5: generic host terminal transport

1. Add Docker exec attach and resize behind a tested runtime interface.
2. Add attachment IDs, bounded queues, cleanup, and tmux-client reconciliation.
3. Add internal `agent_host` status/control/WebSocket routes.
4. Verify simultaneous clients, mixed dimensions, service restart, and stale
   client cleanup.

Exit criterion: generic terminal clients can attach concurrently through
`agent_host` without duplicate processes or phantom clients.

### Phase 6: public transport

1. Add `client_service` terminal handlers and mode validation.
2. Add the bounded WebSocket proxy and framing protocol.
3. Add upgrade errors, close codes, origin checks, and reconnect semantics.
4. Add Nginx WebSocket upgrade handling.

Exit criterion: a generic client can create, ensure, attach, resize, detach,
reattach, stop, and resume using only `client_service`.

### Phase 7: web UI

1. Add CLI navigation, session rail, agent picker, status, and actions.
2. Add xterm rendering, resize, mouse, Unicode, WebGL fallback, and cleanup.
3. Add tmux copy-mode UX and reconnect behavior.
4. Add VS Code and shared workspace/session actions.
5. Add light/dark screenshot fixtures and deterministic terminal mock state.

Exit criterion: the browser acceptance cases pass without regressing Chat.

### Phase 8: hardening and documentation

1. Exercise process/container/host loss, missing secrets/config/state, queue
   overflow, stale labels, and orphan cleanup.
2. Verify credential redaction and trust-boundary documentation.
3. Add `docs/TERMINAL_PROTOCOL.md`.
4. Update `docs/OVERVIEW.md`, `docs/PLAYWRIGHT.md`, README configuration, and
   operations guidance.
5. Run all repository and Docker-gated validation.

## Test Plan

### Configuration and storage

- Aggregate and per-resource YAML round trips for `cli`.
- Unknown CLI fields and unsupported harnesses fail.
- Missing CLI connection references report their exact field path.
- CRUD/export/bundles retain CLI settings.
- SQLite upgrades existing databases without data loss.
- Existing sessions default to Chat mode.
- Launch snapshots and harness session IDs survive service restart.
- API/SQLite session rows never contain resolved secret values.

### Shared Copilot builder

- First and later launches use the same `--session-id`.
- Connection URL/key/flavor map to the correct provider variables.
- `chat_completions` explicitly selects `completions`.
- No connection leaves Copilot in GitHub-auth mode.
- Inherited provider variables cannot override selected configuration.
- Chat and CLI build matching provider/model/path/profile semantics.
- Model, effort, mouse, no-auto-update, and extra args remain separate argv
  entries.
- System prompt creates/selects the owned profile; empty prompt removes stale
  owned state.
- Concurrent sessions see exactly their enabled skills.
- Secret variable names are passed to Copilot redaction/filtering.

### Stable runtime and cleanup

- Full session labels appear on containers and volumes.
- Create/ensure adopts only matching labeled resources.
- Service restart adopts a running container.
- Missing container reuses the stable workspace volume.
- Legacy records return a typed non-adoptable state.
- Explicit cleanup reports before deleting.
- Cleanup never removes a resource owned by a durable session.
- `stack-down` removes exited containers and managed orphan volumes.

### Tmux and terminal controller

- Duplicate concurrent ensure creates one tmux session.
- Pane status distinguishes running, exited status, and missing.
- Ensure reports started, attached, and resumed accurately.
- Disconnecting all clients does not stop Copilot.
- Stop preserves durable session identity.
- Scrollback uses tmux only; no capture replay occurs.
- Copy-mode endpoint enters history safely.
- Copilot receives mouse events while tmux mouse handling is off.
- Fixed Unicode sample preserves cursor placement across resize/reattach.

### WebSocket and host runtime

- Input/output remain raw bytes.
- Resize reaches the correct Docker exec PTY with bounded dimensions.
- Two clients receive output and may send input.
- Mixed sizes use the documented smallest-client policy.
- Normal close detaches the correct tmux client.
- Restart reconciliation removes phantom clients.
- Attachment count is observed from tmux.
- Slow-client overflow detaches only that client with code `4429`.
- Upgrade errors and post-upgrade close codes match the protocol.
- Chat endpoints reject CLI sessions and terminal endpoints reject Chat.

### Web UI

- Agent picker excludes agents without CLI capability.
- New session uses `interaction_mode=cli`.
- xterm sends binary input and resize controls and accepts binary output.
- Session changes dispose every terminal resource.
- Reconnect occurs only for retryable causes and stops after success/unmount.
- Dark/light terminal themes update correctly.
- WebGL failure falls back without detaching.
- Scrollback action enters copy mode.
- VS Code uses the selected runtime's browser-reachable URL.
- Navigation detaches without stopping.
- Stop and Delete remain distinct.
- Started/attached/resumed states are visibly distinct.

### Automated integration coverage

Commit an in-process fake terminal runtime for service contract tests and an
opt-in Docker suite covering:

- tmux creation and duplicate ensure;
- PTY I/O and resize;
- simultaneous attachment;
- mixed-size clients;
- service restart/adoption;
- stale attachment cleanup;
- process and container loss;
- stable workspace reuse;
- Copilot session-ID create/resume compatibility; and
- Unicode width/cursor behavior.

### End-to-end acceptance matrix

| Case | Automation |
| --- | --- |
| Start Copilot through an OpenRouter connection | Docker integration plus manual smoke |
| Colors, paste, alternate screen, mouse, resize | Browser/PTY tests plus manual visual check |
| Emoji/CJK/combining-width correctness | Automated fixed sample plus manual visual check |
| Two browsers type into one live session | Docker/WebSocket integration plus manual smoke |
| Close all browsers and reattach from another device | Automated disconnect/reattach; cross-device manual |
| Restart web UI and `client_service` | Automated service integration |
| Restart `agent_host` and adopt live tmux | Automated Docker integration |
| Lose process/container and resume same Copilot UUID | Automated Docker integration |
| Reboot host and resume workspace/Copilot state | Manual destructive smoke |
| Open VS Code on the same session workspace | Route/UI test plus manual smoke |
| Delete session and remove managed runtime/workspace | Automated integration |

## Validation Gates

Use targeted checks during each phase. Before completion run:

```text
just agent-host-check
just client-service-check
just webui-lint
just webui-screenshots
just check
```

Also run the new Docker-gated terminal integration suite on both Docker and
rootless Podman where available.

No phase may weaken Rust linting, Python strict typing/linting, TypeScript
checks, strict config validation, or existing Chat behavior.

## Principal Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Browser disconnect kills Copilot | Tmux owns the PTY; WebSockets are attachments only. |
| Service restart loses runtime registry | Stable IDs, full labels, idempotent ensure, and adoption. |
| Host reboot loses exact screen | State the boundary and resume the same Copilot UUID/workspace. |
| Duplicate recovery starts two processes | Atomic named tmux creation; process lock is optimization only. |
| Dead pane is mistaken for live | Observe `pane_dead`, exit status, and PID. |
| Phantom clients constrain size | Attachment identity plus tmux-client reconciliation. |
| Mixed browser sizes corrupt display | `window-size smallest`; document padding on larger clients. |
| Scrollback duplicates or redraws alt screen | Tmux-only history, xterm scrollback zero, no capture replay. |
| Tmux steals Copilot mouse input | Tmux mouse off; Copilot mouse on. |
| Unicode width tables diverge | UTF-8 locale, supported tmux version, deliberate xterm widths, fixed tests. |
| One slow browser exhausts memory | Bounded queues and attachment-only overflow close. |
| Secrets appear in children/output | Resolve late, clear inherited provider vars, secret-env filtering, redact logs. |
| Session artifacts pollute saved workspace | Owned relative-path exclusions preserving user `.github` files. |
| Non-destructive shutdown leaks disk | Full labels, explicit reconciliation/cleanup, corrected `stack-down`. |
| Chat and CLI provider behavior diverges | One shared Python Copilot launch/provider module. |
| Terminal API is exposed remotely | Trusted-interface binding, Origin checks, deployment warning; no false auth claim. |

## Definition of Done

CLI View is complete when:

- a CLI-enabled agent starts Copilot through an existing Connection;
- Chat and CLI share the same tested Copilot provider/session semantics;
- multiple browsers can interact with and reattach to one exact live terminal;
- service restarts preserve the live terminal;
- process/container/host loss resumes the same durable Copilot session and
  workspace;
- terminal liveness, recovery kind, attachments, and failures are observed and
  reported explicitly;
- VS Code opens the same session workspace;
- cleanup removes only explicitly owned managed resources;
- the public terminal protocol is documented and client-neutral; and
- all repository, screenshot, and Docker-gated validation passes without
  regressing Chat.
