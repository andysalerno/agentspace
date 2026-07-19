# Session Tools and Automatic Fresh-Session Handoff

## Goal

Add an opt-in builtin skill, `start-fresh-session`, that lets an agent recognize
when the latest user message clearly begins an independent conversation and ask
AgentSpace to process that same message in a fresh session.

The first command in a new Rust CLI will be:

```sh
session-tools start-new
```

The command must be safe to invoke from inside the kernel container while a turn
is running. The user must not need to repeat the message, the gateway-visible
session ID must remain stable, and the old kernel must not answer the message
after requesting the handoff.

## Current behavior

### User-initiated `/new`

Discord serializes inbound messages with `_send_lock`, dispatches slash commands
before sending ordinary messages to the agent, and returns immediately when a
command is handled
(`gateways/gateway_discord/src/gateway_discord/discord_gateway.py:381-410`).
Its `/new` handler calls `client_service`'s reset endpoint and then posts
`a new session has started`
(`gateways/gateway_discord/src/gateway_discord/discord_gateway.py:502-563`).
The `/new` message is control input; it is not delivered to the agent.

`POST /sessions/{session_id}/reset` asks `agent_host` to reset its current
session, replaces the stored upstream `agent_host_session_id`, clears the
client-service message history, and preserves the stable client-facing
`session_id` (`services/client_service_rs/src/api.rs:1167-1190`).

`agent_host` implements reset by destroying the existing kernel container and
creating a new one with the same harness, environment, skills, paths, and
workspace mounts (`services/agent_host_rs/src/sessions.rs:307-322`). This is the
behavior the agent-initiated operation should reuse.

### Turn ownership and streaming

`client_service` owns:

- the stable client session ID and its mapping to the current upstream session
  (`services/client_service_rs/src/models.rs:620-672`);
- persisted user and assistant messages
  (`services/client_service_rs/src/store/sqlite.rs:101-139`);
- the one-active-turn-per-session guard
  (`services/client_service_rs/src/api.rs:2276-2315`); and
- the response stream observed by gateways and the web UI
  (`services/client_service_rs/src/api.rs:2318-2477`).

Both streaming and synchronous message APIs register an active turn
(`services/client_service_rs/src/api.rs:2208-2273,2623-2660`). The Discord
gateway and web UI use the streaming API, while `cli_channel` and the echo
gateway currently use the synchronous API.

Kernel containers already receive `AGENTSPACE_AGENT_ID` and
`AGENTSPACE_CLIENT_SERVICE_URL`
(`services/client_service_rs/src/api.rs:2111-2156`) and share the Compose network
with `client_service`. The kernel image already builds and installs the Rust
`memory` CLI, which is the packaging precedent for `session-tools`
(`kernels/kernel_host/Dockerfile:3-30`).

Builtin skills are discovered from `mounts/skills`, copied into the shared
skills volume, and mounted only when enabled on an agent
(`services/agent_host_rs/src/skills.rs:518-580`). Therefore the new skill should
remain opt-in, consistent with the existing `memory` builtin.

## Recommended architecture

`client_service` should remain the session lifecycle authority. An
agent-created session that only calls `POST /sessions` would be orphaned from
the gateway, and calling the existing reset endpoint directly from inside the
kernel would destroy the container while its CLI request and agent turn are
still running.

Instead, use a deferred, turn-correlated handoff:

```text
latest user message
  -> client_service starts turn in old upstream session
  -> agent invokes `session-tools start-new`
  -> CLI authenticates to a client_service control endpoint
  -> endpoint marks the active turn for one fresh-session replay
  -> CLI returns success
  -> client_service observes the next upstream event, stops the old stream,
     resets the upstream session, clears old history, and recreates records for
     the triggering message
  -> client_service replays the same message to the new upstream session
  -> fresh agent response continues on the original client response stream
```

This keeps the stable client session and gateway mapping unchanged while
ensuring that the current message, rather than only a future message, benefits
from the fresh context.

### Control capability

Generate the stable client `session_id` before creating the upstream session and
add these internal values to the kernel environment:

```text
AGENTSPACE_SESSION_ID
AGENTSPACE_SESSION_CONTROL_TOKEN
AGENTSPACE_CLIENT_SERVICE_URL  # already present
```

Store only a hash of the random control token in the client session record and
SQLite schema. Do not include either the token or its hash in session summaries,
details, traces, or logs. Keep the same capability for the lifetime of the
stable client session so `agent_host` can preserve it through resets along with
the rest of the session environment.

Add an internal endpoint such as:

```http
POST /internal/session-control/start-new
Authorization: Bearer <AGENTSPACE_SESSION_CONTROL_TOKEN>
Content-Type: application/json

{"session_id":"<AGENTSPACE_SESSION_ID>"}
```

The endpoint should:

1. authenticate the capability against the named session;
2. require that the session currently has an active turn;
3. atomically mark that active turn as `start_new_requested`;
4. be idempotent when the same turn requests it again; and
5. return `202 Accepted` before any reset begins.

The handler must not hold the current `std::sync::Mutex` guard across an
`.await`. Missing or invalid capabilities should not reveal whether a session
exists. A request made outside an active turn should fail explicitly.

### Turn handoff

Extend the active-turn state with the restart request. The runner should check
the flag after every upstream event and again when the upstream stream ends or
errors.

Because the CLI's HTTP request completes before the harness can emit its tool
result, the first event observed after the flag is set is a safe point to stop
draining the old stream. At that point:

1. stop forwarding and collecting old-session events;
2. drop the old upstream stream;
3. invoke a shared internal reset helper rather than recursively calling the
   HTTP handler;
4. reload the client session to obtain the new `agent_host_session_id`;
5. clear the old persisted history;
6. create fresh user and assistant message IDs for the replay;
7. append the original user text to the cleared history;
8. stream the same text to the new upstream session; and
9. finalize the original client request with the new assistant response.

The client-facing `turn_id` and response stream can remain stable, but the final
payload should identify that one automatic restart occurred. A regular kernel
event such as `agentspace/session-restarted` can also be emitted so observers
can discard any transient pre-handoff rendering without introducing a third
top-level NDJSON frame type.

Allow at most one automatic replay per client turn. The fresh agent sees no
previous substantive conversation, so the skill must also prohibit invoking
itself when no prior topic exists. If reset or replay fails, return a failed
final payload; never fall back to an answer generated from the old context.

Refactor the streaming and synchronous paths to use the same restart-aware turn
orchestrator. Accepting the control request but ignoring it in `run_turn` would
make the feature silently fail for `cli_channel` and the echo gateway.

### CLI contract

Create a workspace crate at `services/session_tools_rs` with a binary named
`session-tools`. Keep the initial surface deliberately small:

```text
session-tools start-new
session-tools start-new --json
session-tools --help
```

`start-new` reads the service URL, stable session ID, and capability from the
environment, sends the authenticated request with a bounded timeout, and exits:

- `0` only when the handoff was accepted;
- `2` for missing or invalid local configuration; and
- `1` for server, authentication, transport, or protocol failures.

Human-readable success output should tell the agent that the triggering message
will be replayed and that it must stop the old response. `--json` should provide
a stable machine-readable result for future integrations. The CLI must never
print the capability.

Build the crate in the kernel image and copy the resulting binary to
`/usr/local/bin/session-tools`, following the multi-stage `memory` build.

### Skill decision policy

Add `mounts/skills/start-fresh-session/SKILL.md` with frontmatter similar to:

```yaml
---
name: start-fresh-session
description: Use when the latest user message clearly starts an independent conversation and prior context is irrelevant or harmful; keep the current session when uncertain.
---
```

The instructions should make continuity the default:

Invoke `session-tools start-new` only when all of these are true:

- there is an established prior topic or task in this session;
- the latest message begins a clearly independent conversation;
- a useful response does not depend on prior messages; and
- carrying prior context forward creates a meaningful risk of confusion,
  irrelevant assumptions, or wasted context.

Do not invoke it merely because:

- time has passed;
- the user asks a tangent, side question, clarification, or follow-up;
- the topic shifts but may reasonably return to the earlier task;
- the message refers to prior people, files, decisions, pronouns, or results;
- preserving continuity is harmless; or
- the session is already fresh and has no established prior topic.

When uncertain, stay in the current session. When invoking it, do so before
producing user-facing text. After the command succeeds, stop the old response
and do not answer the user; `client_service` will replay the message. If the
command fails, surface the failure rather than pretending a fresh session was
created.

Examples should contrast:

- **Start fresh:** last night's completed home-automation request followed the
  next morning by an unrelated weather question.
- **Keep context:** a brief unrelated question in the middle of an active coding
  task when the user may return to that task.
- **Keep context:** any request that references earlier discussion or artifacts.
- **Keep context:** the first substantive request in a new session.

## Alternatives considered

### Have the CLI create a new client session

Rejected. The gateway retains the old stable client session ID, so the new
session would be unreachable or would require gateway-specific reassignment.
It would also duplicate channel metadata and change the established `/new`
contract.

### Call the existing reset endpoint directly

Rejected. `agent_host` reset destroys the current kernel container. The
`session-tools` process and the agent turn invoking it run in that container, so
the caller can be terminated before the request completes, and the triggering
message cannot be replayed on the existing response stream.

### Handle everything inside `kernel_host`

Rejected. `kernel_host` can reset its inner harness, but it does not own the
stable client session, persisted client message history, active-turn record, or
gateway response stream. A local-only reset would leave those layers
inconsistent and would not provide the intended future home for cross-session
search and resume operations.

### Add a special kernel protocol event as the only control mechanism

Not recommended for the first implementation. A local kernel-host endpoint and
new protocol event could avoid a network capability, but future `session-tools`
commands such as search and resume need `client_service` data anyway. A
dedicated authenticated client-service control API gives the CLI one durable
boundary. A restart event may still be emitted for observer visibility after
the request is accepted.

### Wait for the old model to end its turn before resetting

Rejected. That relies on model compliance, can add unbounded latency, and can
waste tokens on output that must be discarded. The server can safely interrupt
after observing the first post-request upstream event.

## Milestones

Each milestone should be independently reviewed, validated with `just check`,
and committed before starting the next.

### Milestone 1: Session control identity and API

**Scope**

- Generate the stable client session ID before upstream session creation.
- Generate a cryptographically random session-control capability.
- Persist its hash in in-memory and SQLite session stores, including migration
  behavior for existing databases.
- Inject the stable ID and plaintext capability into the upstream kernel
  environment.
- Add restart-request state to `ActiveTurnRecord`.
- Add the authenticated, idempotent `start-new` control endpoint.

**Tests**

- Session creation injects the expected internal environment without exposing
  the capability in API responses.
- SQLite round-trips the capability hash.
- Missing, malformed, wrong-session, and wrong-token requests are rejected
  without session enumeration.
- Requests outside an active turn are rejected.
- Repeated requests for the same active turn return the same accepted result.
- The active-turn mutex is released before asynchronous work.

**Acceptance**

An authenticated request can mark exactly the current turn for restart, but no
existing client behavior changes until the turn orchestrator is added.

### Milestone 2: Restart-aware turn orchestration

**Scope**

- Extract the existing reset logic into a shared internal helper used by both
  `/sessions/{session_id}/reset` and automatic handoff.
- Refactor synchronous and streaming turns onto one restart-aware upstream event
  loop.
- Interrupt the old stream at the first event observed after the request flag.
- Reset, reload the new upstream ID, clear history, allocate fresh message IDs,
  and replay the original user text.
- Preserve the stable client session ID, client turn ID, and streaming
  subscription.
- Emit restart metadata/event and enforce a one-replay limit.
- Make reset/replay failures terminal and explicit.

**Tests**

- The old upstream session is destroyed and a new upstream ID is stored.
- The triggering message is sent once to the old session and once to the new
  session.
- Only the fresh assistant response is persisted and returned.
- Old conversation history and partial old assistant output are removed.
- Streaming subscribers remain attached through handoff.
- Synchronous callers receive the fresh response.
- Upstream end/error immediately after the request still performs the replay.
- A second request during the replay cannot cause a loop.
- Reset failure and replay failure produce failed results without old-context
  fallback.
- A normal turn with no request is byte-for-byte/API compatible with current
  behavior.

**Acceptance**

A test client can request a fresh-session handoff during either message API and
receive the new session's answer through the original request.

### Milestone 3: Rust `session-tools` CLI and kernel packaging

**Scope**

- Add `services/session_tools_rs` to the Cargo workspace.
- Implement `start-new`, environment validation, bounded HTTP requests, safe
  diagnostics, exit codes, and `--json`.
- Build and install `session-tools` in the kernel-host image.
- Add a targeted `just` recipe if one is useful in addition to workspace checks.

**Tests**

- CLI argument and environment parsing.
- Accepted, unauthorized, conflict, malformed-response, timeout, and connection
  failure behavior against a mock server.
- Output and logs never contain the capability.
- Kernel image build contains an executable
  `/usr/local/bin/session-tools` whose `--help` succeeds.

**Acceptance**

An enabled agent can execute `session-tools start-new` from its kernel container
and receive an accepted handoff response.

### Milestone 4: Builtin `start-fresh-session` skill

**Scope**

- Add `mounts/skills/start-fresh-session/SKILL.md`.
- Encode the conservative decision policy, examples, command sequence, stop
  behavior, loop prevention, and failure behavior described above.
- Document that builtin skills are opt-in and that existing agent sessions must
  be reset after enabling the skill.

**Tests**

- Builtin skill synchronization recognizes the new skill as read-only.
- Its frontmatter name matches its directory and description.
- A kernel session with the skill enabled receives its instructions; a session
  without it does not.

**Acceptance**

The skill appears in the Skills UI, can be enabled on an agent, and gives the
agent an unambiguous conservative rule for invoking the CLI.

### Milestone 5: End-to-end gateway verification and observability

**Scope**

- Add an integration test covering a persistent gateway session with prior
  history, an unrelated latest message, automatic handoff, and a fresh answer.
- Confirm Discord ignores or appropriately records the restart event while
  continuing to consume the same stream.
- Confirm web UI stream handling does not mistake restart metadata for a final
  frame.
- Add structured logs and counters for requested, completed, failed, and
  loop-prevented handoffs without recording prompts or capabilities.
- Document operational troubleshooting and the relationship between `/new` and
  agent-initiated handoff.

**Tests**

- Stable gateway/client session ID before and after handoff.
- New upstream kernel ID after handoff.
- No lost or duplicate triggering message.
- No old-context assistant reply reaches the user.
- Existing user-issued `/new` behavior remains unchanged.
- Full `just check` and a Podman kernel image/stack smoke test.

**Acceptance**

Discord and web sessions can grow across unrelated conversations while enabled
agents conservatively move clearly independent messages to fresh kernel context
without user intervention.

## Implementation invariants

- `client_service` remains the source of truth for client sessions and history.
- `agent_host` remains the source of truth for kernel container lifecycle.
- The gateway-visible client session ID never changes during reset or handoff.
- The triggering user message is never lost and is replayed at most once.
- Old-session assistant content is never accepted as the answer after handoff.
- Capabilities are scoped to one stable client session and never exposed.
- A model cannot create a reset loop within one user turn.
- Failure is explicit; there is no silent fallback to stale context.
- Existing `/new` behavior and ordinary turns remain compatible.
