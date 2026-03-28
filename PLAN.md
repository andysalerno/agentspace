# AgentSpace — Architecture Plan

## Services & Projects

Below is the proposed set of projects in this repo, each a distinct service (or library). They communicate over swappable transports (gRPC, REST, or in-process), defined by shared interface contracts.

---

### 1. `proto/` — Shared Interface Definitions

Protobuf (or similar) definitions that define the contracts between all services. Even if a given deployment uses REST or in-process calls, these definitions serve as the canonical API spec.

This is the source of truth. Everything else implements or consumes these interfaces.

---

### 2. `kernel/` — Kernel Interface & Implementations

The innermost layer. A kernel wraps a headless agent harness and exposes a uniform streaming interface.

**Responsibility:**
- Shim over a specific agent CLI (Claude Code, Copilot CLI, Codex, etc.)
- Spawn the underlying process, capture streaming stdout/stderr
- Translate between the common kernel interface and the harness-specific I/O
- Mount and expose skills to the inner harness
- Report status (idle, busy, error, dead)
- **Session management is internal to the harness** — the kernel's streaming output includes a session ID (near the start of a new session) that can be used to resume later; session data is persisted in the kernel container's private data dir in an opaque, harness-specific format
- One kernel container = one session; the container lives as long as the session does

**Proposed implementations (each a sub-project or module):**
- `kernel-claude-code/`
- `kernel-copilot-cli/`
- `kernel-codex/`
- Others as needed

**High-level API surface:**

```
Kernel.Start(config) -> session stream (includes session_id emitted by the harness)
Kernel.Resume(session_id, config) -> session stream (resumes existing session)
Kernel.Send(session_id, message) -> ack
Kernel.StreamOutput(session_id) -> stream of events (text chunks, tool calls, status changes)
Kernel.Stop(session_id) -> ack
Kernel.Status(session_id) -> status
```

**Language:** Python initially (these are wrappers around CLI processes, Python is fine here).

---

### 3. `agent-host/` — Agent Host Service

The orchestrator. Owns kernel lifecycle and skill management.

**Responsibility:**
- Spawn and destroy kernel instances (as containers or processes) — **one kernel container per session**
- Map agent definitions → kernel config (which kernel type, which skills, system prompt / personality)
- Volume-mount enabled skills into kernel containers
- Track active sessions and their state; each kernel container has a private data dir where the inner harness persists its own session state in an opaque format
- Route messages to/from the correct kernel instance
- Support multiple read-only observers attaching to a kernel instance's raw output stream (fan-out)
- Handle **session reset** — destroy the current kernel container for a session and spawn a fresh one (new session ID), e.g. when a user issues `/reset` in a channel
- Enforce resource limits and concurrency policies

**High-level API surface:**

```
AgentHost.CreateSession(agent_id) -> session_id
AgentHost.SendMessage(session_id, message) -> stream of output events
AgentHost.ListSessions() -> list of session summaries
AgentHost.GetSession(session_id) -> session detail + status
AgentHost.DestroySession(session_id) -> ack
AgentHost.ResetSession(session_id) -> new session_id (destroys kernel, spawns fresh one)
AgentHost.Attach(session_id) -> read-only stream of raw kernel output events
```

**Language:** Python to prototype, Rust long-term.

---

### 4. `client-service/` — Client-Facing API Gateway

The service that all clients (web, CLI, integrations) talk to. Translates user-facing concerns into agent-host calls.

**Responsibility:**
- CRUD for **agent definitions** (name, personality, enabled skills, enabled channels, per-channel instructions)
- Full CRUD for **skills** — a skill is a folder/zip containing `SKILL.md` and optionally scripts; client-service stores, validates, and serves them to agent-host for mounting
- Manage **chat sessions** (create, list, resume, history)
- Own the full lifecycle of **channels** — create, configure, start, stop, and destroy channel instances (as processes or containers); track their status and per-channel instructions
- Persist agent definitions, skills, channel configs, session history, and user data
- Serve as the single entry point — clients never talk to agent-host directly

**High-level API surface:**

```
# Agent management
Agents.Create(agent_def) -> agent
Agents.Get(agent_id) -> agent
Agents.Update(agent_id, agent_def) -> agent
Agents.Delete(agent_id) -> ack
Agents.List() -> list of agents

# Skills (a skill = folder/zip with SKILL.md + optional scripts)
Skills.Create(name, archive) -> skill
Skills.Get(skill_id) -> skill detail + contents
Skills.Update(skill_id, archive) -> skill
Skills.Delete(skill_id) -> ack
Skills.List() -> list of skill summaries (name, description)

# Chat
Chat.Start(agent_id, channel?) -> session
Chat.Send(session_id, message) -> stream of output events
Chat.History(session_id) -> list of messages
Chat.ListSessions(agent_id?) -> list of sessions

# Channels (client-service owns lifecycle; channel processes are dumb relays)
Channels.ListTypes() -> list of available channel types (discord, matrix, irc, ...)
Channels.Create(channel_type, config, agent_id, instructions?) -> channel instance
Channels.Get(channel_id) -> channel detail (status, config, instructions)
Channels.Update(channel_id, config?, instructions?) -> channel instance
Channels.Delete(channel_id) -> ack
Channels.List(agent_id?) -> list of channel instances + statuses
Channels.Start(channel_id) -> ack
Channels.Stop(channel_id) -> ack

# Kernel observability (read-only attach to raw streaming output)
Kernels.List() -> list of active kernel sessions + status
Kernels.Attach(session_id) -> read-only stream of raw kernel output events
```

**Language:** Python to prototype, Rust long-term.

---

### 5. `client-web/` — Web Client

A browser-based UI.

**Responsibility:**
- Create and configure agents (select personality, skills, channels)
- Chat with agents in real time (streaming responses)
- View and manage active sessions
- **`/kernels` page** — a real-time grid/dashboard of all running kernel sessions; each tile shows the live raw streaming output of that kernel, read-only (attach via `Kernels.Attach`)

**Language:** TypeScript (React, Svelte, or similar).

---

### 6. `client-cli/` — CLI Client

A terminal-based UI.

**Responsibility:**
- Same capabilities as the web client, but in a terminal
- Interactive chat with streaming output
- Agent management via commands or TUI
- **Attach mode** — read-only tail of a running kernel session's raw output (equivalent of the web `/kernels` view)

**Language:** Python (prompt_toolkit or similar) or Rust (ratatui).

---

### 7. `channels/` — Channel Relay Implementations

Dumb relay processes that bridge an external platform to the client-service. They contain no business logic — all configuration, per-channel instructions, lifecycle, and state live in client-service. A channel process receives its config on startup and simply relays messages.

**Sub-projects:**
- `channels/discord/`
- `channels/matrix/`
- `channels/irc/`

**Responsibility:**
- Connect to the external platform using config provided by client-service
- Listen for messages directed at a configured agent
- Relay messages to client-service via the `Chat.*` API, stream responses back to the platform
- That's it — no state, no instructions logic, no lifecycle management

Client-service spawns and kills these as processes or containers. If a channel dies, client-service can detect it and optionally restart it.

**Language:** Python (most platform SDKs are Python-first).

---

### 8. `skills/` — Bundled / Example Skills

A directory of example or built-in skill packages that ship with the repo. Each skill is a directory containing `SKILL.md` and optionally scripts/tools.

**Structure:**
```
skills/
  some-skill/
    SKILL.md          # name, description, instructions for the agent
    tools/            # any tool definitions, scripts, etc.
  another-skill/
    SKILL.md
    tools/
```

In production, skills are managed via the `Skills.*` API on client-service (uploaded as folders or zip archives). Client-service stores them and provides them to agent-host, which volume-mounts the enabled skills into kernel containers. This directory exists for development convenience and as starter examples.

---

### 9. `store/` — Persistence Library

A shared library for data persistence, used by client-service (and potentially agent-host).

**Responsibility:**
- Store and retrieve agent definitions, session history, skill metadata
- Abstract over storage backends (SQLite for dev, Postgres for prod, etc.)

**Language:** Matches the services that use it (Python initially).

---

## Deployment

```
                    ┌─────────────┐
                    │  client-web │
                    └──────┬──────┘
                           │
┌────────────┐      ┌──────▼──────┐      ┌────────────┐
│ client-cli ├──────► client-svc  ◄──────┤  channels   │
└────────────┘      └──────┬──────┘      └────────────┘
                           │
                    ┌──────▼──────┐
                    │  agent-host │
                    └──────┬──────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         ┌─────────┐ ┌─────────┐ ┌─────────┐
         │kernel(1)│ │kernel(2)│ │kernel(n)│
         └─────────┘ └─────────┘ └─────────┘
```

- Everything runs via `docker-compose.yml` at the repo root
- Each service is its own container
- Kernels are spawned as sibling containers by the agent-host
- Skills directory is volume-mounted into kernel containers
- Can also run outside containers for local dev (in-process transport)

---

## Open Questions

- ~~**Observability:**~~ **Resolved.** No separate service. Client-service exposes `Kernels.List()` and `Kernels.Attach(session_id)` for read-only streaming. Clients (web `/kernels` grid, CLI attach mode) connect to these to observe agents in real time.
- ~~**Multi-tenancy:**~~ **Resolved.** Single-user, self-deployed on private hardware. No auth or tenant isolation needed for now.
- ~~**Skill discovery:**~~ **Resolved.** Skills are local directories only. No registry or marketplace.
- ~~**Session persistence:**~~ **Resolved.** Each kernel container manages its own session internally — the inner agent harness (Claude, Codex, etc.) owns the persistence format, which is opaque to us. One kernel container per session. The kernel's streaming output includes a session ID that agent-host captures and can use to resume. Channels get one persistent session for their lifetime; a `/reset` command from the user triggers agent-host to destroy the kernel container and spawn a new one with a fresh session.
