# Client/Server Slice Plan

## Purpose

This plan covers the next vertical slice:

- one `client_service` gateway that is the only service clients talk to
- one minimal `webui` service that talks to `client_service`
- one future `cli_ui` client that also talks to `client_service`

This slice should stay intentionally small. The goal is to establish the correct client/server boundary and prove the end-to-end flow, not to build the full product surface yet.

## Where The Repo Stands Now

Implemented today:

- `kernel`
- `kernel_echo`
- `kernel_copilot`
- `kernel_host`
- `agent_host`

Working flow today:

- `agent_host` spawns one `kernel_host` container per session
- each `kernel_host` container runs the actual harness
- `copilot-cli` is the only real harness path
- `agent_host` exposes a small HTTP session API

Missing from the original architecture:

- `client_service`
- `webui`
- `cli_ui`
- `store`
- `channels`
- `proto`
- streaming attach / observer fan-out

## Comparison To Original PLAN.md

### What still matches

- The overall layering is still correct:
  - clients on top
  - one client-facing service in the middle
  - `agent_host` below that
  - kernels below `agent_host`
- `agent_host` is correctly responsible for kernel lifecycle, not user-facing concerns.
- kernels still own harness-specific session persistence.

### Where we deviated

- We built `agent_host` before `client_service`.
  - This was a reasonable sequencing choice because the kernel/session lifecycle had to be proven first.
- We used direct HTTP contracts instead of introducing `proto/` first.
  - This is acceptable for now, but the APIs should be kept small and easy to formalize later.
- `agent_host` currently returns buffered event lists for `send_message`.
  - The original plan assumes streaming output and attach semantics; that is still not implemented.
- Session metadata in `agent_host` is still in-memory.
  - The original plan expects persistence to exist higher in the stack.

### Changes needed to the original plan

No major architectural change is needed. The plan should be updated only in sequencing:

1. keep `agent_host` as the kernel/session orchestrator
2. add `client_service` next as the single public API
3. put both `webui` and `cli_ui` above `client_service`
4. postpone `proto`, `store`, channels, and real-time attach until after the first client-facing slice works

## Guiding Decisions For This Slice

### 1. `client_service` is the only public backend API

Neither `webui` nor `cli_ui` should talk to `agent_host` directly.

### 2. Keep persistence minimal for the first pass

Start with in-memory state inside `client_service`, just enough to model:

- agent definitions
- chat sessions mapped to `agent_host` sessions
- chat transcript history

Do not block this slice on `store/`.

### 3. Keep the web UI intentionally simple

The web UI only needs enough functionality to exercise the gateway:

- create or select one agent
- start a chat session
- send a message
- show the assistant response
- list prior sessions

No advanced design work is needed yet.

### 4. Preserve clean boundaries

- `webui` owns browser concerns only
- `client_service` owns user-facing API, state shaping, and session mapping
- `agent_host` owns kernel lifecycle only

### 5. Design for later streaming, but do not require it yet

The first version may use request/response message sends. The API shape should leave room to add streaming later without rewriting the whole stack.

## Proposed Project Layout

```text
services/
  client_service/
    pyproject.toml
    Dockerfile
    compose.yaml
    .env.example
    run-service.sh
    run-service.ps1
    src/
      client_service/
        __init__.py
        app.py
        service.py
        models.py
        agent_host_client.py
    tests/
      test_service.py
      test_app.py

clients/
  webui/
    package.json
    Dockerfile
    compose.yaml
    src/
      ...

  cli_ui/
    pyproject.toml
    src/
      cli_ui/
        ...
```

Notes:

- `client_service` belongs under `services/`.
- `webui` and `cli_ui` belong under `clients/`, because they are not orchestration services.
- The repo `uv` workspace will need to include `clients/cli_ui` only if that client is Python-based and added in this slice.

## Scope For `client_service`

### Responsibilities in this slice

- define user-facing agent records
- provide session CRUD for chat sessions
- map client-facing sessions to `agent_host` session IDs
- proxy chat requests to `agent_host`
- persist chat transcript in memory
- expose a simple HTTP API for web and CLI clients

### Explicitly out of scope for this slice

- skills CRUD
- channel lifecycle
- kernel attach streaming
- authentication
- durable database persistence
- multi-user concerns

## Minimal Data Model

### Agent

Initial shape:

- `agent_id`
- `name`
- `harness`
- `system_prompt`

Notes:

- `harness` should use the same strongly typed harness enum concept already introduced lower in the stack.
- only `copilot-cli` needs to be supported initially
- `system_prompt` may be stored even if the lower layers do not fully apply it yet

### Chat Session

Initial shape:

- `session_id`
- `agent_id`
- `agent_host_session_id`
- `status`
- `created_at`
- `updated_at`

### Chat Message

Initial shape:

- `message_id`
- `session_id`
- `role` (`user` or `assistant`)
- `content`
- `created_at`

For the first pass, the assistant message can be reconstructed from returned `text_delta` events and stored as a single flattened string.

## Proposed `client_service` API

### Health

- `GET /healthz`

### Agents

- `POST /agents`
- `GET /agents`
- `GET /agents/{agent_id}`
- `PATCH /agents/{agent_id}`
- `DELETE /agents/{agent_id}`

### Chat Sessions

- `POST /sessions`
  - input: `agent_id`, optional `cwd`
  - behavior: create an `agent_host` session and store the mapping
- `GET /sessions`
- `GET /sessions/{session_id}`
- `GET /sessions/{session_id}/messages`
- `POST /sessions/{session_id}/messages`
  - input: user message
  - behavior:
    - append user message locally
    - call `agent_host`
    - flatten assistant `text_delta` output
    - append assistant message locally
    - return both raw events and flattened assistant text
- `POST /sessions/{session_id}/reset`
- `DELETE /sessions/{session_id}`

### Transport to `agent_host`

`client_service` should talk to `agent_host` through a small typed client wrapper:

- `create_session`
- `send_message`
- `get_session`
- `list_sessions`
- `history`
- `reset_session`
- `destroy_session`

This isolates `client_service` from raw HTTP details and keeps the boundary easy to swap later.

## Web UI Plan

### Goal

Build the smallest hosted UI that proves:

- browser -> `webui`
- `webui` -> `client_service`
- `client_service` -> `agent_host`
- `agent_host` -> `kernel_host`
- `kernel_host` -> `copilot-cli`

### Recommended shape

Use the existing dashboard shape.

Preferred implementation:

- TypeScript/React single-page app for `webui`
- Vite build served by Nginx
- client-side API calls to `client_service`

Reasoning:

- aligns with the current dashboard architecture
- easy to containerize beside the service containers
- keeps all browser-facing code in the web UI package

### Initial pages

#### `/`

Landing page with:

- list of agents
- form to create an agent
- list of sessions
- button to open a session

#### `/sessions/{session_id}`

Chat page with:

- session header
- transcript
- message form
- reset button

### UX rules for this slice

- no websocket requirement
- no streaming requirement
- standard form-post / fetch update is acceptable
- keep styling simple and readable

## CLI UI Plan

Do not implement full CLI UI in the same first pass unless time remains.

What this slice should preserve for it:

- `client_service` endpoints should be usable from a terminal client without web-specific assumptions
- assistant responses should be returned as plain text as well as raw events

The headless CLI channel should be able to:

- list agents
- create a session
- send a message
- print the reply

## Execution Order

### Phase 1: `client_service` foundation

1. scaffold the client service under `services/client_service_rs`
2. add typed models for agents, sessions, and messages
3. add an `AgentHostClient` wrapper
4. implement in-memory `ClientService`
5. add HTTP routes
6. add tests

### Phase 2: `client_service` e2e

1. add Dockerfile and compose/run scripts
2. run it against the existing `agent_host`
3. verify create agent -> create session -> send message -> read transcript

### Phase 3: minimal `webui`

1. scaffold `clients/webui`
2. add simple server-rendered pages
3. call `client_service` over HTTP
4. verify browser flow for agent creation and chat

### Phase 4: integration cleanup

1. add docs and example curl commands
2. add root-level compose guidance for bringing up the stack in order
3. decide whether a tiny `cli_ui` should be added immediately or deferred

## Testing Plan

### Automated

For `client_service`:

- service-layer tests with a stub `AgentHostClient`
- API contract tests against the service router
- validation tests for transcript flattening and reset behavior

For `webui`:

- route tests for rendered pages
- API/client wrapper tests if a wrapper module is added

### Manual e2e

Required manual path:

1. start `agent_host`
2. start `client_service`
3. start `webui`
4. create an agent
5. create a session
6. send two messages
7. confirm second reply preserves Copilot context through the lower stack
8. bring all containers down after validation

## Risks And Follow-Ups

### 1. `system_prompt` may not yet be applied end to end

That is acceptable for the first `client_service` slice, but the model should include it so the interface does not need to change later.

### 2. Transcript persistence will diverge from kernel raw history

That is acceptable for now. `client_service` should treat its transcript as the user-facing conversation view, not the canonical raw kernel log.

### 3. Reset semantics need to be explicit

When `client_service` resets a session, it should:

- call `agent_host.reset_session`
- update the mapped `agent_host_session_id`
- keep the same client-facing session ID if possible
- clear local transcript or mark a reset boundary

My recommendation for the first pass:

- keep the same client-facing session ID
- clear transcript and record that a reset occurred

### 4. Web UI should not depend on browser-only state

Keep the first version simple enough that a future CLI client can use the exact same `client_service` API without special cases.

## Definition Of Done For This Slice

This slice is complete when:

- `client_service` exists and is the only API used by clients
- `webui` can create an agent, start a session, send messages, and view history
- `client_service` stores agent records and transcript history in memory
- `client_service` proxies chat to `agent_host`
- the full Dockerized path works with real `copilot-cli`
- tests pass
- docs explain how to launch and exercise the stack

## Recommended Next Step After This Slice

After the first client/server slice works, the next highest-value follow-up is:

1. add durable persistence via `store`
2. add streaming chat and kernel attach support
3. add a minimal `cli_ui`
4. then move on to channels and broader agent definition features
