# Agent Memory Plan

## Summary

AgentSpace should add a text-first memory system with three parts:

1. A built-in `memory` skill that teaches agents when and how to use memory.
2. A single Rust `memory` binary whose CLI uses the same client/service path
   through either an in-process transport or HTTP.
3. A Memory page in the Web UI, backed by `client_service`, for browsing,
   searching, editing, and linking the same memories agents use.

The first store is intentionally simple: a persistent container volume
containing Markdown files with YAML frontmatter. Directories form a concept
tree, while relative Markdown links form a graph. Querying is deterministic
filesystem scanning over paths, titles, tags, links, and body text. There is no
database, generated index, vector store, or embedding model in the first
version.

The initial corpus is installation-scoped and shared by every agent that has
the `memory` skill enabled. Agent and session IDs are recorded as provenance,
not used as access controls. This matches the proposed single volume and keeps
the first contract small. Per-agent or per-user stores can later be added as
new volume scopes without changing page or CLI semantics.

## Design Principles

The implementation should follow existing AgentSpace boundaries:

- `client_service` remains the only public backend used by clients and the Web
  UI.
- `agent_host` remains responsible for kernel lifecycle and container mounts,
  not memory content.
- Built-in skills continue to live under `mounts/skills` and flow through the
  existing skill registry.
- Persistent container storage uses stable named volumes, as Workspaces and
  GitAgent already do.
- Kernel capabilities are opt-in through agent configuration. A kernel only
  receives the memory volume when `memory` is in its enabled skills.
- The core model is usable without the full stack. `memory` defaults to a local
  directory and only becomes an HTTP client when `AGENTSPACE_MEMORY_URI` is
  set.
- Local and remote modes share models, validation, query semantics, and
  conformance tests. HTTP is a transport adapter, not a second implementation.
- Files remain human-readable, portable, greppable, and recoverable with normal
  filesystem and volume backup tools.
- Invalid input and unavailable backends fail explicitly. There is no fallback
  from a configured remote backend to local storage.

## Proposed Architecture

```text
                         public API
Web UI  ----------------------------------------------+
                                                      |
                                                      v
                                                client_service
                                                      |
                                                      | internal HTTP
                                                      v
                                              memory --serve
                                                      |
                                                      v
                                            agentspace-memory-data
                                                named volume
                                                      ^
                                                      |
                  local mode                          | same files
kernel + memory skill + memory CLI -------------------+

                  remote mode
kernel + memory skill + memory CLI -- HTTP ----------> memory --serve
             AGENTSPACE_MEMORY_URI
```

The control-plane `memory --serve` container always mounts the memory volume so
the Web UI can manage it. The opt-in requirement applies to hosted agent
kernels: only a kernel with the `memory` skill enabled receives the volume
needed by the local transport. The path is an internal implementation detail:
it is not advertised as an accessible workspace path or documented in the
skill. A deployment may set `AGENTSPACE_MEMORY_URI` for an agent to use the
server instead; the CLI then ignores its local directory.

The implementation should be a Rust workspace crate at `services/memory_rs`.
It produces one binary named `memory`; that same artifact runs normal CLI
commands and the server selected by `memory --serve`. The kernel image and the
memory service image copy the same compiled binary rather than maintaining
separate CLI and server packages.

The crate contains:

- transport-neutral request, response, page, and error models;
- a `MemoryClient` trait used by every CLI command;
- direct and HTTP implementations of `MemoryClient`;
- a `MemoryService` containing all application behavior;
- a `MemoryStore` trait and filesystem implementation beneath the service;
- the Clap CLI;
- the Axum HTTP adapter used by `memory --serve`.

This follows the existing Rust service pattern, workspace lints, and
`just rust-check`. Reuse established Rust dependencies where possible and add a
maintained Serde-compatible YAML parser for frontmatter rather than
implementing a partial YAML parser.

### One client/server path in both modes

Local mode must not be a separate CLI implementation that reaches into files
directly. After argument parsing, every command constructs the same typed
request and calls the same `MemoryClient` interface:

```text
local:
CLI command -> DirectMemoryClient -> MemoryService -> FilesystemMemoryStore

remote:
CLI command -> HttpMemoryClient -> HTTP -> MemoryService
                                      -> FilesystemMemoryStore
```

`DirectMemoryClient` is an in-process transport adapter: it owns or references
the same `MemoryService` used by the server and delegates typed calls directly.
`HttpMemoryClient` only serializes those request/response models over HTTP.
Axum handlers deserialize requests, call `MemoryService`, and serialize the
same responses. Validation, query behavior, revisions, locking, link updates,
and domain error mapping live behind `MemoryService` and are therefore
identical in both modes.

Transport code may handle connection failures, HTTP status mapping, and
serialization, but must not implement memory behavior. The test suite should
run one client contract against `DirectMemoryClient` and `HttpMemoryClient` to
prevent the two paths from drifting.

### Agent-facing filesystem abstraction

Every supported agent interaction with memory goes through the `memory` binary.
The skill must not tell the agent where the corpus is mounted or suggest direct
filesystem access. Structured reads and writes use commands such as
`memory read`, `memory write`, `memory query`, `memory move`, and `memory rm`.
Familiar read-oriented filesystem inspection is available only through:

```text
memory run <command> [args...]
```

The initial executable allowlist is:

```text
rg ls cat head tail wc stat pwd
```

`memory run` executes the selected program directly with the corpus root as its
working directory. It does not invoke a shell and does not support an inner
pipeline, redirection, substitution, or shell built-in. An outer shell may
still consume its output, for example:

```sh
memory run rg birthday | head
```

After checking only the executable name, the implementation passes all
remaining arguments through unchanged. This deliberately preserves familiar
tool behavior, but makes `memory run` a convenience boundary rather than a
security boundary: absolute paths and tool-specific escape hatches such as
`rg --pre` are not blocked in v1, and `memory run pwd` reveals the backing path.
The subprocess should still receive a minimal environment and the memory
service container must contain no credentials. Hard confinement would require
strict per-command argument validation or an OS-level read-only sandbox and is
an explicit future hardening option.

Both transports use a shared `AllowlistedCommandRunner` behind
`MemoryService`. Local mode spawns the process in the kernel; remote mode spawns
it in the memory service container. The allowlist, environment construction,
timeout, output limits, cancellation, and exit semantics are application
behavior shared by both modes.

## Memory Model

### Files and paths

The configured store root is not proactively advertised to the agent. The
examples use `<memory-root>` only to explain the on-disk format; its actual path
is not part of the stable agent-facing contract even though `memory run pwd` or
other passthrough arguments can reveal it. A page path is its canonical
identity:

```text
<memory-root>/
  people/
    alice.md
  projects/
    agentspace.md
  decisions/
    memory-format.md
```

CLI and API paths are root-relative and may omit the `.md` suffix. They must:

- use UTF-8;
- resolve to a Markdown file beneath the store root;
- reject absolute paths, `.` and `..` segments, empty segments, control
  characters, and symlink traversal;
- use `/` as the logical separator on every transport;
- have bounded path, page, tag, and request sizes.

Directories are implicit. Empty directories do not need to be retained.

### Frontmatter

Every page uses canonical YAML frontmatter:

```markdown
---
schema_version: 1
title: Alice
tags:
  - birthday
  - person
created_at: 2026-07-17T06:35:09Z
updated_at: 2026-07-17T06:35:09Z
created_by: research-agent
updated_by: research-agent
---

Alice's birthday is ...

Related: [AgentSpace](../projects/agentspace.md)
```

`schema_version`, `title`, `tags`, `created_at`, and `updated_at` are required.
The `*_by` provenance fields are optional and should use
`AGENTSPACE_AGENT_ID` when available. Unknown frontmatter fields are preserved
so the format can evolve additively.

Tags are normalized lowercase values, deduplicated, and sorted when a page is
written through the CLI or API. Links are ordinary relative Markdown links to
other `.md` pages. They are derived from page content and are not duplicated in
frontmatter or an index.

The store accepts manually created valid Markdown pages for administrator
recovery and migration. The skill must direct agents to use the structured
commands for mutations and `memory run` for familiar filesystem inspection so
timestamps, revisions, locking, and link maintenance remain under the memory
service contract.

### Revisions and concurrent access

The page revision is a deterministic digest of the exact stored bytes. Reads
return that revision. Updates and deletes accept an optional expected revision;
the Web UI always supplies it and receives `409 Conflict` if the page changed.

Filesystem mutations must:

1. acquire a store-wide advisory lock shared by local CLI and server processes;
2. validate the current revision when one was supplied;
3. write a temporary file in the destination directory;
4. flush and atomically rename it into place;
5. release the lock.

`memory move` updates relative inbound links while holding the same lock, then
commits all changed files or leaves the store unchanged. The first version
should not claim to preserve external links, absolute URLs, reference-style
links, or links in arbitrary HTML.

### Query and graph behavior

The first version scans the filesystem on each operation. This avoids stale
indexes and keeps the authoritative files trustworthy.

- Text query is case-insensitive literal matching over path, title, tags, and
  body.
- Tag filters use exact normalized tag matching.
- Prefix filters restrict results to a subtree.
- Results have a deterministic path sort and configurable limit.
- Link queries return outgoing links, resolved targets, broken links, and
  backlinks.
- `memory check` reports invalid frontmatter, unsafe paths, duplicate normalized
  tags, and broken internal links.

The service/store boundary should leave room for a future indexed or
embedding-based store implementation, but those features must not alter these
baseline semantics.

## CLI Contract

The CLI should be useful interactively and predictable for agent scripting:

```text
memory --help
memory write <path> [--title TITLE] [--tag TAG]... [--file FILE]
memory read <path>
memory move <source> <destination> [--if-revision REVISION]
memory rm <path> [--if-revision REVISION]
memory pages ls [--under PREFIX] [--with-tag TAG]...
memory query <text> [--under PREFIX] [--with-tag TAG]... [--limit N]
memory tags ls
memory links <path> [--backlinks]
memory check
memory run <rg|ls|cat|head|tail|wc|stat|pwd> [args...]
memory --serve [--host HOST] [--port PORT]
```

`memory write` reads the Markdown body from `--file` or standard input. It
creates missing parent directories and replaces an existing page only when
explicitly requested or when its expected revision matches. Commands support a
stable `--json` output for agents; human-readable output remains concise.

`memory run` is the only passthrough form; shorthand commands such as
`memory rg` are not supported. Direct mode and remote mode stream stdout and
stderr independently and make the `memory` process exit with the invoked
command's exit code. Both apply configurable execution-time and output-byte
limits. A timeout, output-limit termination, cancellation, transport failure,
or rejected executable is distinguishable from the child process's own exit.

Backend selection precedence is:

1. explicit `--uri` or `--root`;
2. `AGENTSPACE_MEMORY_URI`;
3. `AGENTSPACE_MEMORY_DIR`;
4. the private built-in local root.

`--uri` and `--root` are mutually exclusive. If a remote URI is configured,
connection and protocol errors are surfaced and local storage is never used as
a success-shaped fallback. Hosted-agent documentation and the built-in skill
must not expose `--root`, `AGENTSPACE_MEMORY_DIR`, or the built-in root; those
remain operator and local-development configuration.

## HTTP Contract

`memory --serve` exposes a versioned internal API:

```text
GET    /healthz
GET    /v1/pages
GET    /v1/pages/content?path=<path>
PUT    /v1/pages/content?path=<path>
DELETE /v1/pages/content?path=<path>
POST   /v1/pages/move
GET    /v1/tags
GET    /v1/links?path=<path>
GET    /v1/check
POST   /v1/run
```

List/query parameters mirror the CLI. Page reads return metadata, body,
outgoing links, and revision. Mutations accept `expected_revision` and return
the resulting page or a structured conflict. Error responses distinguish
validation errors, missing pages, conflicts, unavailable storage, and internal
failures.

`POST /v1/run` accepts an argument vector, validates its first element against
the executable allowlist, and returns a streaming response. Frames preserve
arbitrary stdout and stderr bytes separately and end with the child exit code
or a typed timeout, output-limit, cancellation, or launch error. The HTTP
client writes decoded chunks to its own stdout/stderr as they arrive and exits
with equivalent semantics. The protocol must bound both execution duration and
total streamed bytes, kill the child when a bound is exceeded or the client
disconnects, and never buffer unbounded command output.

`client_service` proxies this as `/memory/...`, just as it proxies other
specialized services. Configure its upstream with
`CLIENT_SERVICE_MEMORY_BASE_URL`; the Web UI never calls the memory service
directly. The memory service should be reachable on the internal Compose
network but should not need a host-published port.

Authentication is out of scope until AgentSpace has a general identity and
authorization model. For the first version, the memory service trusts the
private stack network and `client_service` boundary. The skill and UI must warn
that memory is shared and must not contain credentials or other secrets.

## Skill-Associated Volumes

Add optional AgentSpace metadata to a skill in `agentspace.json`. The built-in
memory skill declares:

```json
{
  "schema_version": 1,
  "resources": {
    "volumes": [
      {
        "id": "data",
        "scope": "installation",
        "mount_path": "/var/lib/agentspace/memory",
        "advertise": false,
        "mode": "rw"
      }
    ]
  }
}
```

For v1:

- only built-in skills may declare runtime resources;
- `scope` supports only `installation`;
- actual volume names are derived by AgentSpace, never supplied by a skill;
- resource IDs, mount paths, modes, and collisions are strictly validated;
- reserved kernel paths cannot be shadowed;
- `advertise: false` prevents the mount path from being added to harness
  accessible paths, prompts, skill text, or session summaries;
- the volume is created lazily, labeled as AgentSpace-managed, and retained
  when sessions are reset or deleted;
- enabling multiple skills that request the same mount path fails session
  creation with an actionable error.

The memory resource resolves to the stable named volume
`${AGENTSPACE_MEMORY_VOLUME:-agentspace-memory-data}`. Compose uses the same
volume for `memory --serve`. In local mode, `agent_host` mounts it at the
private built-in path only when `memory` is among the session's enabled skills.

This is a soft abstraction, not filesystem isolation. Because the binary runs
as the same container user, a sufficiently determined agent may discover and
access the mount. The supported interface, skill, prompts, and diagnostics
nevertheless expose only the `memory` CLI. Hard isolation would require moving
local access behind a sidecar/Unix-socket boundary or using a privileged
sandbox, neither of which is part of v1.

This metadata is generic rather than a special `if skill == "memory"` branch.
Future schema versions can add agent-scoped volumes, read-only shared datasets,
quotas, or other managed resources. User-created skills should not gain volume
declarations until storage quotas, lifecycle controls, and trust policy exist.

## Web UI

Add a top-level **Memory** page following the existing Skills and Workspaces
patterns. The first version should provide:

- a collapsible directory tree;
- text search, subtree filtering, and tag filtering;
- a tag list with counts;
- page metadata and Markdown preview;
- create, edit, move, and delete actions;
- outgoing links, backlinks, and visibly broken links;
- an explicit dirty-state warning and revision-conflict handling;
- a shared-memory/no-secrets notice;
- empty, loading, unavailable, invalid-page, and conflict states.

Use Monaco for editing and the existing Markdown rendering stack for previews.
The page should edit frontmatter fields through controls and the body as
Markdown rather than exposing two competing raw-document editors. API types and
query keys belong in the existing `types.ts`, `api.ts`, and `queries.ts`
surfaces. Navigation continues to use the current `ViewId`, `Sidebar`, and
`App` view-switch pattern.

The UI is a view over the authoritative files, not a second store. It must
refresh affected page, tree, tag, and graph queries after mutations.

## Incremental Milestones

### Milestone 1: Filesystem model and local CLI

Deliver a standalone, tested local memory tool before changing container
orchestration.

Work:

- Add `services/memory_rs` to the Cargo workspace with a single `memory` binary
  target and the repository's workspace lint configuration.
- Implement transport-neutral models, `MemoryClient`, `MemoryService`, and
  `MemoryStore` abstractions.
- Implement `DirectMemoryClient` as an in-process adapter over the service.
- Implement `AllowlistedCommandRunner` with direct argv execution, the fixed
  read-oriented allowlist, separate stdout/stderr streaming, exit-code
  preservation, cancellation, and configurable timeout/output limits.
- Implement the filesystem store, locking, atomic writes, revisions, moves,
  queries, tags, links, backlinks, and integrity checks.
- Implement the local forms of all CLI commands except `--serve`.
- Add `--json` output and structured exit codes.
- Add fixtures containing nested concepts, Unicode text, relative links,
  broken links, and invalid pages.
- Include unit and direct-client contract tests in `cargo test` and
  `just check`.

Acceptance criteria:

- A user can create, query, link, move, update, and delete Markdown memories in
  a temporary directory using only `memory`.
- Concurrent/stale writes are rejected rather than silently overwriting data.
- Traversal, symlink escape, malformed frontmatter, and oversize input tests
  fail safely.
- Reopening the directory reconstructs all list, tag, and graph results without
  an index or migration step.
- `memory run` accepts only the eight named executables, never invokes a shell,
  preserves their arguments, streams, and exit codes, and terminates children
  that exceed configured limits.

This milestone is useful independently as a portable local memory manager.

### Milestone 2: Built-in skill and opt-in persistent volume

Make local memory available to hosted agents while introducing generic,
trusted skill resources.

Work:

- Extend the skill model and built-in sync path to parse and expose optional
  `agentspace.json` metadata.
- Resolve enabled built-in skill volume resources during session creation.
- Extend `agent_host` runtime mount construction to validate, create, label,
  and mount those volumes.
- Add the `advertise` resource property and keep non-advertised mount paths out
  of harness accessible paths, prompts, summaries, and diagnostics.
- Add `mounts/skills/memory/SKILL.md` and its resource manifest.
- Teach the skill to inspect `memory --help`, query before writing, prefer
  updating an existing page over duplication, use links/tags sparingly, run
  `memory check`, use `memory run` for common filesystem inspection, never
  access or mention a backing path, and never store secrets.
- Build and install the `memory` binary in the kernel image.
- Declare the stable memory volume in Compose.

Acceptance criteria:

- A session for an agent without the `memory` skill has no memory-data mount.
- A newly created session for an agent with the skill has the persistent volume
  at the non-advertised private path, can run `memory --help`, and retains
  writes across reset, deletion, stack down, and stack up.
- The skill, generated prompts, session API, and harness accessible paths do not
  proactively advertise or recommend the backing filesystem path. The
  passthrough `pwd` command may reveal it by design.
- Two memory-enabled sessions see the same installation-scoped corpus.
- Invalid manifests and mount collisions prevent startup with clear errors.
- Existing skills without metadata and existing workspace mounts behave
  unchanged.

This milestone delivers durable memory to agents without requiring an HTTP
service or Web UI.

### Milestone 3: HTTP transport and public service boundary

Expose the same store remotely without changing CLI behavior or bypassing
`client_service`.

Work:

- Add the Axum adapter and `memory --serve` mode to the same Rust binary used by
  local CLI commands.
- Implement `HttpMemoryClient`, selected by
  `AGENTSPACE_MEMORY_URI`.
- Run one `MemoryClient` contract suite against direct and HTTP transports.
- Implement the framed `/v1/run` stream and verify stdout/stderr ordering per
  stream, arbitrary bytes, exit-code parity, disconnect cancellation, and
  timeout/output-limit termination.
- Add a multi-stage memory service image that runs the same compiled `memory`
  artifact as the kernel image, plus a Compose service sharing
  `agentspace-memory-data`.
- Add health checks, bounded timeouts, request limits, and structured logging.
- Add `client_service` configuration, upstream client, `/memory` proxy routes,
  response models, and error mapping.
- Add client-service route and upstream-proxy tests.
- Document local, in-stack remote, and externally hosted URI configuration.

Acceptance criteria:

- Every CLI operation produces equivalent results in local and HTTP modes.
- The service and a local CLI can safely mutate the shared volume without
  corrupting files.
- `client_service` exposes memory operations while the memory service remains
  private to the stack network.
- Upstream timeout, malformed response, conflict, and unavailable-service cases
  return explicit non-success responses.
- Setting `AGENTSPACE_MEMORY_URI` never silently falls back to the local store.

This milestone enables remote agents, automation, and the future UI while
preserving offline local mode.

### Milestone 4: Memory Web UI and end-to-end release

Deliver the operator-facing memory experience and finish the feature as one
coherent stack capability.

Work:

- Add Memory API types, client methods, React Query keys/hooks, navigation, and
  `MemoryView.tsx`.
- Implement tree browsing, search, tag filters, editor/preview, link graph
  panels, CRUD, move, integrity status, and revision-conflict UX.
- Surface memory service health and distinguish an empty store from an
  unavailable one.
- Update README/architecture documentation, environment examples, and volume
  backup/removal instructions.
- Add end-to-end coverage for enabling the skill, writing from an agent
  session, viewing/editing in the UI, reading the edit from another session,
  and preserving data across stack recreation.
- Run the full repository verification and containerized stack smoke flow.

Acceptance criteria:

- The Web UI and memory-enabled agents observe the same pages, tags, and links.
- A stale browser edit cannot overwrite a newer agent edit.
- Agents without the skill still receive neither the CLI instructions nor the
  memory-data mount.
- Memory-enabled agents are taught only the structured commands and
  `memory run`; no UI or structured agent-facing API proactively advertises the
  backing path, although `memory run pwd` may reveal it.
- Memory survives normal session and stack lifecycle operations.
- `just check` and the containerized end-to-end scenario pass.

This milestone completes the requested local/remote CLI, opt-in agent storage,
and Web UI experience.

## Test Strategy

Tests should be layered around the shared contract:

- **Model tests:** path normalization, frontmatter round trips, tag
  normalization, revision calculation, size bounds, and link resolution.
- **Filesystem tests:** atomicity, locking, stale revisions, moves with
  backlinks, symlink/traversal rejection, manual valid files, and restart
  reconstruction.
- **Client conformance tests:** run the same CRUD/query/tag/graph/error cases
  through `DirectMemoryClient` and `HttpMemoryClient`; both must reach the same
  `MemoryService` behavior.
- **Command-runner tests:** executable allowlist, direct argv preservation, no
  shell parsing, working directory, minimal environment, stdout/stderr
  streaming, arbitrary output bytes, exit parity, cancellation, timeout, and
  output limits in direct and HTTP modes.
- **Agent host tests:** manifest validation, resource resolution, volume labels,
  mount gating, collision rejection, and suppression of non-advertised paths.
- **Client service tests:** route contracts, proxy payloads, timeout/error
  mapping, and no direct memory-service exposure to Web UI code.
- **Web checks:** TypeScript build, lint/dead-code checks, and browser smoke
  coverage using the repository's existing tooling.
- **Stack test:** use Podman locally to prove cross-container visibility and
  persistence; keep Docker in GitHub Actions.

Each milestone ends with `just check`. Container behavior should additionally
be verified with the smallest relevant Compose build and smoke scenario.

## Operational and Security Rules

- Never follow symlinks or allow paths outside the configured root.
- Apply root/path confinement to structured page operations. `memory run`
  intentionally passes arguments through unchanged and is not a security
  boundary in v1.
- Execute `memory run` without a shell, with a minimal environment, explicit
  timeout/output limits, child cleanup on cancellation, and an unprivileged
  service container containing no credentials.
- Never render raw HTML from memory pages without the Web UI's existing
  sanitization boundary.
- Apply explicit limits to page size, request size, result count, and scan
  duration.
- Do not log page bodies, frontmatter values, or query text at normal log
  levels.
- Do not automatically delete the memory volume when disabling the skill,
  deleting an agent, deleting a session, or running `stack-down`.
- Document an explicit administrator command for backup and intentional volume
  deletion.
- Treat the corpus as shared, non-secret data until AgentSpace has general
  authentication and authorization.
- Keep timestamps and generated metadata in UTC.
- Use additive schema changes and preserve unknown frontmatter fields.

## Explicit Non-Goals for Version 1

- Embeddings, semantic/vector search, reranking, or model-managed retrieval.
- Automatic memory extraction from every chat turn.
- Hidden system-prompt injection of memories.
- Per-user, per-agent, or organization ACLs.
- A database or generated search index.
- Automatic deduplication, summarization, forgetting, or retention policies.
- Page version history beyond volume-level backups.
- Arbitrary host bind mounts or user-supplied volume names in skill metadata.
- User-created skills provisioning persistent volumes.
- Hard filesystem isolation from the kernel user.
- Sandboxing or per-command argument validation for `memory run`.

These can be added later behind the client, service/store, and resource-scope
abstractions without replacing the Markdown corpus or changing the basic CLI.
