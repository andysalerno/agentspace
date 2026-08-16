# Information from Copilot CLI Telemetry

## Status

Implementation plan based on GitHub Copilot CLI `1.0.81-0`, its built-in
`copilot help monitoring` documentation, the OpenTelemetry GenAI semantic
conventions, and local JSONL captures made on 2026-08-15.

## Goal

Collect structured, near-real-time information from an interactive Copilot CLI
session without reading or interpreting terminal output.

The first UI should show a compact usage summary in CLI View:

- last interaction and whole-session token usage;
- input and output tokens;
- cache-read and cache-write tokens;
- cache reuse percentage;
- model-call, tool-call, and subagent counts;
- current context occupancy when Copilot reports it; and
- telemetry health and freshness.

The design must also support a later, dismissible tree view containing:

- one top-level node per user interaction;
- the agent's final response;
- model calls made while producing it;
- tool calls in execution order;
- nested subagent invocations and their own model/tool activity; and
- optional tool arguments, tool results, prompts, and responses.

The compact summary must not require sensitive content capture. Rich content
must be an explicit later opt-in.

## Non-Goals

- Screen scraping, ANSI parsing, or deriving state from terminal text.
- Sending terminal stdout or stderr through a telemetry parser.
- Treating human-oriented debug logs as a stable data contract.
- Running an OTLP collector or requiring local TLS certificates in the first
  implementation.
- Replacing Copilot's own `/usage` or `/context` UI.
- Claiming an exact system-prompt token count when Copilot does not report one.
- Claiming that a prompt-cache break is certain when it is only inferred.
- Capturing prompts, source code, shell output, or tool payloads by default.
- Exposing Copilot-specific OpenTelemetry attribute names above `kernel_host`.

## Executive Decisions

1. Use Copilot's supported OpenTelemetry JSONL file exporter.
2. Force content capture off in the first release.
3. Store raw JSONL in a private, per-session telemetry volume, not the shared
   Copilot home or the user workspace.
4. Parse and normalize Copilot records inside `kernel_host`.
5. Keep telemetry transport separate from PTY WebSocket frames.
6. Use completed `chat` spans as the authoritative unit of token usage.
7. Use span parentage, not file order, to reconstruct interactions and
   subagents.
8. Expose nullable values and reporting coverage; never turn "not reported"
   into a misleading zero.
9. Call the cache metric "cache reuse" or "input served from cache," not
   request hit rate.
10. Treat cache-break detection as a labeled inference unless Copilot emits an
    explicit event.
11. Do not add exact system-prompt tokens to the initial UI because that value
    is unavailable.
12. Design the public model around interactions, agent invocations, model
    calls, and tool calls so another CLI can implement the same contract
    without emitting Copilot's schema.

## Enabling the Exporter

AgentSpace should add these values to the managed Copilot launch environment:

```text
COPILOT_OTEL_ENABLED=true
COPILOT_OTEL_EXPORTER_TYPE=file
COPILOT_OTEL_FILE_EXPORTER_PATH=/var/lib/agentspace/telemetry/<launch-id>.jsonl
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=false
OTEL_RESOURCE_ATTRIBUTES=agentspace.session.id=<durable-session-id>
```

Setting the file path alone enables telemetry, but the explicit enable and
exporter type make intent unambiguous.

These keys become reserved Copilot-launch keys. Agent or connection
environment values must not silently redirect managed telemetry to another
file or endpoint. Reject conflicting values during launch-snapshot validation,
or overwrite them with an explicit warning. The managed path must never
contain user input.

Use a new file for every Copilot process launch and read all JSONL files in the
session directory. This avoids relying on open-mode behavior and preserves
earlier process generations. The installed exporter was observed appending
when two processes used the same path, but per-launch files are safer and make
partial-file recovery simpler.

Do not use an OTLP sidecar in v1. Copilot `1.0.81-0` refuses cleartext
`http://` OTLP endpoints, including localhost, and silently disables export
apart from a process-log warning. A collector would therefore require TLS or
extra certificate management, while the file exporter is local and structured.

## What the JSONL Contains

### Record envelope

The file contains one JSON object per line. The useful record types are:

- `type: "span"` for agent, model, plan, and tool activity; and
- `type: "metric"` for cumulative or histogram metric snapshots.

A span normally contains:

```text
traceId
spanId
parentSpanId
name
startTime
endTime
attributes
events
status
resource
instrumentationScope
```

Timestamps are high-resolution `[unix_seconds, nanoseconds]` pairs. Children
are often written before parents because records are exported when spans end.
The parser must therefore accept orphan children and attach them when their
parent arrives. JSONL order is completion order, not tree order.

### Observed trace hierarchy

An ordinary interaction is:

```text
invoke_agent
  chat <model>
  execute_tool <tool>
  chat <model>
  ...
```

A locally captured subagent interaction on `1.0.81-0` was:

```text
invoke_agent
  chat gpt-5.6-sol
  execute_tool task
    invoke_agent task
      chat claude-haiku-4.5
  chat gpt-5.6-sol
```

All nodes shared one trace ID. The nested `invoke_agent task` reported
`gen_ai.agent.id=builtin:task` and `gen_ai.agent.name=task`. This is sufficient
to count subagent invocations, attribute their descendant model usage, and
render them as nested nodes.

Copilot's monitoring help also documents a nested `plan` span:

```text
invoke_agent
  plan
    chat <model>
    execute_tool <tool>
```

Plan-mode JSONL still needs a repository test fixture before implementation.
The normalized tree must preserve unknown intermediate span kinds instead of
assuming every child is directly under an agent.

### Interaction and call identities

Useful identities include:

- `traceId`: one trace tree for an agent interaction;
- `spanId` and `parentSpanId`: stable tree edges within that trace;
- `gen_ai.conversation.id`: the durable Copilot conversation/session ID;
- `github.copilot.interaction_id`: an interaction ID on model calls;
- `github.copilot.turn_id`: the model round number within an interaction;
- `gen_ai.response.id`: a provider response ID;
- `gen_ai.request.previous_response.id`: a provider-dependent response-chain
  link; and
- `gen_ai.tool.call.id`: a tool-call identity.

`github.copilot.turn_id` is not a human-message number. One human interaction
can contain many model rounds. Public API and UI names must use:

- **interaction** for the user-to-agent operation;
- **model call** or **model round** for a `chat` span; and
- **subagent invocation** for a nested `invoke_agent` span.

Do not expose raw trace IDs as domain meaning. They may be used as opaque
stable node IDs.

## Token Accounting

### Per-model-call fields

Observed attributes are:

| Normalized field | Copilot attribute | Notes |
| --- | --- | --- |
| `input_tokens` | `gen_ai.usage.input_tokens` | Includes cache-read and cache-write input. |
| `output_tokens` | `gen_ai.usage.output_tokens` | Includes billed/generated output. |
| `reasoning_output_tokens` | `gen_ai.usage.reasoning.output_tokens` | Provider-dependent subset/detail of output. |
| `cache_read_input_tokens` | `gen_ai.usage.cache_read.input_tokens` | Input served from a prompt/KV cache. |
| `cache_write_input_tokens` | `gen_ai.usage.cache_creation.input_tokens` | Input written into a cache. |

The parser should also accept the observed ecosystem alias
`gen_ai.usage.cache_write.input_tokens` for cache writes.

In a local GPT capture:

```text
input                    16,473
cache read               16,412
cache creation               58
ordinary uncached input       3
```

This confirms that cache categories are subdivisions of input, not additional
tokens.

Derived values are:

```text
total_tokens = input_tokens + output_tokens

uncached_input_tokens =
  input_tokens - cache_read_input_tokens - cache_write_input_tokens

cache_reuse_percent =
  cache_read_input_tokens / input_tokens * 100
```

Clamp an invalid negative uncached result to unknown and report a parser
warning rather than silently coercing malformed source data.

Reasoning tokens must not be added to `output_tokens` again. They are a
provider-dependent detail of output accounting.

### Aggregation rules

Use `chat` spans for exact usage totals:

- session totals: sum every unique completed `chat` span in the session;
- interaction totals: sum leaf `chat` spans under the top-level
  `invoke_agent`;
- subagent totals: sum `chat` descendants of nested `invoke_agent` spans; and
- last-call totals: use the newest completed `chat` span by end time.

Do not add `invoke_agent` usage to its child `chat` usage. Agent spans repeat
aggregates and would double-count.

Do not sum repeated metric snapshots. The `gen_ai.client.token.usage` metric is
useful for observability backends but is less suitable than spans for exact
per-call accounting and can be exported repeatedly with cumulative state.

Deduplicate spans by `(source file identity, traceId, spanId)`. If a future
Copilot version repeats a span across files, additionally deduplicate model
calls by response ID when present, while retaining a diagnostic conflict if
the reported values differ.

### Missing and provider-dependent values

Missing cache fields do not always mean zero. Providers and models differ in
what they report.

Each model call should carry a cache-reporting state:

```text
reported     at least one cache-read/write field was present
unreported   neither field was present
```

When reporting is `reported`, a missing read or write component may be treated
as zero for that call. A session cache percentage is fully authoritative only
when every included call reports cache semantics. The summary must therefore
include reporting coverage:

```text
cache_reported_calls
model_calls
cache_reuse_percent: number | null
```

The WebUI may show a partial percentage only in expanded details and label its
coverage. It must not present a partial value as a whole-session fact.

### System prompt size

Copilot does not report a separate system-prompt token count.

With content capture disabled, the JSONL contains neither the system text nor
its token count. With content capture enabled, it contains
`gen_ai.system_instructions`, but still does not provide the exact tokenization
of that field. Input also includes messages, tool definitions, attachments,
and other context.

The initial UI must therefore not show "system prompt tokens." It should show:

- current context tokens;
- context limit; and
- context occupancy percentage;

when the `github.copilot.session.usage_info` event provides:

```text
github.copilot.current_tokens
github.copilot.token_limit
github.copilot.messages_length
```

A future content-enabled view may show system-instruction characters and UTF-8
bytes. It must not label a local tokenizer estimate as an exact provider token
count.

## Prompt/KV Cache Behavior

### What is explicit

Each reporting model call can provide:

- cache-read tokens;
- cache-write/creation tokens; and
- ordinary uncached input by subtraction.

These values directly show how much of that request reused cached input. They
do not directly explain why reuse changed.

### Cache-break detection

The tested JSONL did not emit an explicit `cache_break` event. A cache reset or
break can be inferred but not proven.

Examples of evidence:

- cache-read tokens collapse after previously high reuse;
- cache-write tokens rise to most of the current input;
- context occupancy drops sharply;
- the model changes;
- a compaction or truncation event is emitted; or
- a provider response chain restarts.

Only the first three are present consistently enough to use across providers.
A model change should be classified as an expected cache boundary, not a
failure.

The normalized model should expose raw per-call cache values immediately. A
later derived signal may use:

```text
cache_reset_suspected
confidence: low | medium
reason:
  reuse_collapsed
  context_discontinuity
  compaction_or_truncation
```

Compare only consecutive calls on the same best-effort execution lane:

```text
conversation + nearest agent identity + model
```

Require a prior call with meaningful cache reuse before classifying a collapse.
A first call, a new subagent, an unsupported provider, or missing cache fields
must produce `not_applicable` or `unknown`, not a cache-break warning.

Recommended initial inference:

- prior comparable call reused at least 50% of input;
- current comparable call reuses less than 10%; and
- current cache writes are at least 50% of input.

These thresholds are product heuristics, not protocol facts. Store the raw
values and compute the label in the telemetry adapter so it can evolve without
a database migration.

If a future Copilot version emits explicit compaction, truncation, or cache
events in OTel, preserve them as interaction events and prefer that evidence.
Do not parse Copilot's private session files merely to make this inference.

## Subagents

Subagent invocations are observable and structurally attributable.

Classification:

- a root `invoke_agent` without a parent is the top-level agent invocation;
- a nested `invoke_agent` is a subagent invocation;
- `gen_ai.agent.id`, `gen_ai.agent.name`, and
  `github.copilot.agent.type` identify it when present; and
- descendant `chat` and `execute_tool` spans belong to that subagent.

The locally observed `task` invocation carried its own aggregate usage and a
child model call. As with the root agent, count only the child `chat` spans for
tokens.

Expose at least:

```text
subagent_invocations
subagent_model_calls
subagent_input_tokens
subagent_output_tokens
subagent_cache_read_input_tokens
subagent_cache_write_input_tokens
subagent_duration_ms
```

Session totals include subagent calls. Subagent totals are a breakdown, not an
additional amount.

Subagent support must remain tolerant:

- names and IDs are optional;
- nested subagents may recurse;
- background subagents may complete out of order;
- a subagent may use another model;
- children may arrive before their parent; and
- context propagation may create an intermediate tool span.

Never infer subagent execution from
`github.copilot.context.custom_agent_names`; that field lists available agents,
not agents that ran.

## Tool Usage

### Content capture disabled

An `execute_tool <tool>` span provides:

- tool name;
- tool type;
- tool-call ID;
- conversation, trace, span, and parent identities;
- start and end time;
- duration;
- status; and
- provider metadata.

This is enough for counts, timing, success/error state when reported, and a
metadata-only tree.

The tested `bash` tool span did not contain arguments or results when content
capture was false.

### Content capture enabled

With:

```text
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true
```

the tested CLI added:

- `gen_ai.input.messages`;
- `gen_ai.output.messages`;
- `gen_ai.system_instructions`;
- full `gen_ai.tool.definitions`;
- `gen_ai.tool.description`;
- `gen_ai.tool.call.arguments`;
- `gen_ai.tool.call.result`; and
- tool-specific parameter attributes.

The captured values were JSON strings, even where their logical shape was an
array or object. The parser must accept both structured JSON values and
stringified JSON.

In a minimal one-tool probe, content capture increased the JSONL file from
18,337 bytes to 262,782 bytes for the same 16 records. Most of the increase was
repeated system instructions and full tool schemas on model spans.

Content capture can persist:

- user prompts and agent responses;
- system instructions;
- source and file contents;
- shell commands and output;
- tool arguments and results;
- repository paths;
- attachments;
- secrets accidentally placed in context; and
- large repeated tool schemas.

Copilot exposes only a boolean switch; it cannot capture messages while
excluding tool payloads at the source. AgentSpace may redact or discard fields
after reading them, but the raw JSONL already contains the original plaintext.

Therefore:

- v1 forces content capture off;
- the future tree initially shows metadata only;
- content capture requires explicit per-agent or per-session opt-in;
- the UI must explain the storage and privacy impact;
- content-enabled files need tighter size limits and retention;
- public APIs must never return raw OTel records; and
- content must be projected into typed message/tool fields after validation.

## Other Extractable Information

### Model and request information

- requested and response model;
- provider name;
- response and previous-response IDs;
- finish reasons;
- streaming flag;
- service request ID;
- server duration;
- time to first chunk; and
- per-output-chunk timing metrics.

The provider name may remain `github` for several model families. Model-family
reporting must use request/response model fields, not infer it from provider.

### Cost and credits

- `github.copilot.cost` is an opaque service-reported cost value;
- `github.copilot.nano_aiu` is AI-credit usage in nano-units; and
- dividing nano-AIU by `1_000_000_000` yields display AI credits.

Do not label `github.copilot.cost` as dollars. Store nano-AIU as an integer
where possible to avoid floating-point drift.

### Timing and status

- model, tool, subagent, and interaction duration;
- server-side model duration;
- time to first chunk;
- span status;
- finish reason; and
- error metadata when a provider emits it.

Detailed tool error text is not guaranteed without content capture. The UI
must distinguish `error reported` from `error details unavailable`.

### Tool and MCP activity

Metrics include:

- model-operation duration and token usage;
- agent duration, inference-call count, and tool-call count;
- tool duration and invocation count;
- agent model-round count; and
- MCP server connection attempts.

Spans remain authoritative for exact tree nodes. Metrics are useful as
cross-checks and for future health dashboards.

### Context and lifecycle events

The tested model spans included `github.copilot.session.usage_info`, containing
current context tokens, token limit, and message count.

Copilot versions may also emit events for compaction, truncation, MCP lifecycle,
skills, and shutdown. Parse known events defensively and preserve unknown event
names internally. Do not require optional lifecycle events for core totals.

### Deliberately excluded source attributes

Copilot emits `enduser.pseudo.id` even when content capture is false. It is not
needed by AgentSpace and must be discarded during normalization.

Do not expose:

- raw resource attributes;
- instrumentation internals;
- end-user pseudonymous IDs;
- service request IDs in the normal UI;
- raw response IDs; or
- arbitrary unknown attributes.

They may appear only in local diagnostic logs with the same redaction policy as
other runtime metadata.

## Architecture

### Data flow

```text
Copilot CLI
  -> per-launch JSONL in a per-session telemetry volume
  -> CopilotOtelTelemetryAdapter in kernel_host
  -> normalized snapshot and update stream
  -> agent_host proxy
  -> typed client_service API
  -> WebUI summary and future tree
```

The PTY path remains:

```text
Copilot CLI <-> tmux <-> terminal WebSocket <-> xterm
```

There is no connection between PTY bytes and telemetry parsing.

### Harness boundary

Add a small kernel-side protocol:

```python
class SessionTelemetryProvider(Protocol):
    async def snapshot(self) -> TelemetrySnapshot: ...
    async def interactions(
        self,
        *,
        cursor: str | None,
        limit: int,
    ) -> TelemetryPage: ...
    async def updates(self) -> AsyncIterator[TelemetryUpdate]: ...
```

Implement `CopilotOtelTelemetryProvider` for `copilot_cli`.

Only this adapter knows:

- Copilot environment-variable names;
- dotted OTel attribute names;
- Copilot span and event names;
- cache-write aliases;
- duplicate aggregate behavior;
- stringified content fields; and
- Copilot-specific fallback rules.

The Rust services and WebUI consume normalized names only.

Unsupported harnesses return a typed `unavailable` capability, not an empty
successful snapshot.

### File reader

The provider owns a singleton directory reader per kernel process:

- discover `*.jsonl` files in the private telemetry directory;
- read only complete newline-terminated records;
- preserve a partial final line until more bytes arrive;
- track file identity and byte offset;
- detect truncation or replacement;
- bound line size and total indexed nodes;
- parse new lines incrementally;
- replay all files after `kernel_host` restart;
- tolerate malformed or unknown records;
- surface a degraded health state and error count; and
- never stop Copilot because telemetry failed.

Use filesystem notifications when reliable, with a short bounded polling
fallback. "Real time" means after a span closes and the exporter writes it,
not token-by-token. A local persistent-interactive probe produced a completed
model span while Copilot remained alive within five seconds.

### Durable raw storage

Add a labeled, per-session Docker volume mounted read-write only into its kernel
container:

```text
agentspace-session-telemetry-<stable-suffix>
  -> /var/lib/agentspace/telemetry
```

Do not use:

- `/root/.copilot`, because it is shared and would mix sessions;
- `/workspace`, because telemetry is AgentSpace state and must not enter saved
  workspaces; or
- the container writable layer, because runtime recreation would lose data.

`agent_host` owns creation, adoption, collision checks, orphan cleanup, and
session-deletion cleanup for this volume. Give it the durable AgentSpace
session label and a distinct managed role.

Persist `telemetry_volume_identity` in the durable client session, even though
the first Docker implementation can derive its volume name. This keeps the
durable contract independent of one runtime's naming convention and makes
adoption and cleanup explicit. Existing rows receive `None`, meaning telemetry
history is unavailable until a managed runtime creates it. Do not infer
telemetry recoverability from a missing field.

The raw JSONL is the replay log. Do not duplicate every span into the
`client_service` SQLite database in v1. This keeps the high-volume,
harness-specific source outside the normalized session store while still
surviving browser, service, container, and pane restarts.

### Normalized model

Use versioned transport models. Suggested summary:

```text
TelemetrySnapshot
  schema_version
  state: disabled | starting | live | stale | unavailable | degraded
  content_capture: false | true
  updated_at
  source_version
  last_interaction: UsageBreakdown | null
  session: UsageBreakdown
  current_context: ContextUsage | null
  counts: ActivityCounts
  cache_signal: CacheSignal | null
  reporting: ReportingCoverage
```

```text
UsageBreakdown
  input_tokens
  output_tokens
  total_tokens
  reasoning_output_tokens
  cache_read_input_tokens
  cache_write_input_tokens
  uncached_input_tokens
  cache_reuse_percent
  nano_aiu
```

Use optional integers for source values. A derived total is optional when its
required components are unavailable.

Suggested tree node:

```text
TelemetryNode
  id
  parent_id
  kind:
    interaction | plan | agent | model_call | tool_call | event | unknown
  name
  started_at
  ended_at
  status
  usage
  model
  agent
  tool
  content: TelemetryContent | null
  children
```

Keep `content` absent in metadata mode. Do not model tool calls as chat
messages; they are child activity of an interaction or agent invocation.

### Internal and public routes

Add kernel routes:

```text
GET /telemetry
GET /telemetry/interactions?cursor=<opaque>&limit=<bounded>
GET /telemetry/stream
```

Proxy equivalent session-scoped routes through `agent_host`.

Expose only through `client_service`:

```text
GET /sessions/{session_id}/telemetry
GET /sessions/{session_id}/telemetry/interactions
GET /sessions/{session_id}/telemetry/stream
```

The stream should use newline-delimited JSON to match existing streaming
infrastructure. It sends:

1. an initial complete snapshot;
2. bounded update events containing a new snapshot revision; and
3. health transitions.

Clients reconnect by fetching a fresh snapshot rather than requiring an
unbounded durable event cursor. Tree pagination uses opaque stable cursors.

Apply the same mode and ownership validation as terminal routes. The route is
named `telemetry`, not `terminal/telemetry`, so it can survive a future
CLI-to-Chat transition and support other harness transports.

Bound:

- JSONL line size;
- stream update frequency;
- interaction page size;
- tree depth;
- content field size;
- total in-memory index size; and
- browser response size.

Coalesce bursts of span completions into one summary update. Never apply
backpressure to Copilot or tmux because a browser is slow.

## WebUI Design

### Initial compact summary

Place a compact usage strip in the CLI header below the identity row. Do not
put it inside the xterm canvas or overload the terminal connection status bar;
that risks repeating the prior terminal-height and FitAddon regressions.

Recommended always-visible values:

```text
Session  48.2k tokens
Last     16.5k in / 13 out
Cache    99.6% (16.4k reused)
Context  17.8k / 272k
Agents   1 subagent
```

At narrower widths, collapse to:

```text
48.2k tokens | 99.6% cache | 1 subagent
```

An expandable details popover should show:

- input, output, and total;
- cache read, cache write, and uncached input;
- cache reporting coverage;
- last interaction versus session;
- model calls and model breakdown;
- subagent usage breakdown;
- tool calls;
- AI credits;
- current context;
- update age; and
- telemetry warnings.

Use "Cache reuse" in labels and tooltips. Explain that it is the share of input
tokens served from cache, not the percentage of requests with any cache hit.

If system-prompt tokens are requested, show "Not reported by Copilot" in
details. Do not put a fabricated estimate in the summary.

### Realtime behavior

Open the telemetry NDJSON stream only for the selected session. Update a
dedicated React Query cache entry from stream snapshots and perform a normal
GET on reconnect or window focus.

Telemetry stream loss must not disconnect the terminal. Show stale values with
an age indicator and retry independently with bounded backoff.

### Future tree

Add a dismissible side panel or wide popover without changing the terminal's
measured container:

- top level: user interactions ordered by start time;
- interaction summary: status, duration, usage, model rounds, tools, and
  subagents;
- children: model, tool, plan, and agent nodes in timestamp order;
- nested agent nodes: recursively expandable;
- metadata mode: names, timing, status, and usage only;
- content mode: user message, final response, tool arguments, and tool result;
  and
- large content: lazy-loaded and truncated with an explicit expansion action.

Because spans finish out of order, display a temporary in-progress placeholder
for a parent that has not yet been exported. Reconcile it by stable span ID
when the parent arrives.

With content capture disabled, the user prompt and final response text are not
available. The tree must say "Content capture disabled" rather than trying to
copy text from xterm.

## Configuration and Privacy

### V1

Telemetry metadata is enabled automatically for supported CLI harnesses.
Content capture is hard-coded false and included in the launch snapshot as
durable intent.

The file permissions and volume are private to the kernel runtime. Raw JSONL is
not exposed through an API, included in workspace snapshots, or written to
normal application logs.

### Future content mode

Add an explicit nested setting only when the tree implementation is ready:

```yaml
spec:
  cli:
    harness: copilot-cli
    telemetry:
      captureContent: false
```

Changing this setting affects new Copilot process launches. A running process
cannot safely change exporter content policy in place.

The UI and config API must warn that enabling it stores prompts, responses,
tool inputs and outputs, source content, and system instructions in plaintext.
The content policy belongs in the durable launch snapshot so recovery does not
silently change it.

Before supporting `true`, add:

- per-field maximums and truncation markers;
- raw-file quota and retention policy;
- an explicit delete-telemetry operation;
- tests proving excluded attributes never reach the public API;
- display escaping for arbitrary terminal/tool content; and
- a decision on whether raw content volumes require encryption at rest.

## Failure Semantics

Telemetry is auxiliary. Failures must be visible but must not stop or restart
the CLI.

Examples:

- exporter file missing while process is starting: `starting`;
- unsupported Copilot version/schema: `degraded`;
- malformed final partial line: wait for completion;
- malformed completed line: increment error count and continue;
- telemetry volume unavailable at launch: start the terminal only if product
  policy explicitly permits `unavailable`; otherwise fail terminal ensure with
  a precise telemetry-storage error;
- kernel unavailable: the API returns `unavailable`; the WebUI retains its
  last snapshot as stale until the durable volume can be replayed by a
  recovered kernel;
- stream disconnected: keep the last UI snapshot and mark it stale; and
- content field rejected by a bound: retain metadata and mark content
  truncated.

Do not return zero-shaped success when telemetry is missing.

## Implementation Phases

### Phase 1: Capture and normalize

1. Add a per-session telemetry volume to Docker runtime lifecycle.
2. Reserve and inject the Copilot OTel environment.
3. Add the kernel telemetry provider protocol.
4. Implement incremental JSONL reading and the Copilot adapter.
5. Normalize spans, hierarchy, tokens, context, tools, subagents, costs, and
   health.
6. Add fixtures from installed `1.0.81-0` with sensitive IDs replaced.
7. Add internal snapshot and paginated interaction routes.

### Phase 2: Public summary and realtime UI

1. Add typed `agent_host` proxy methods and routes.
2. Add typed `client_service` summary and NDJSON stream routes.
3. Add WebUI types, API methods, query keys, and stream handling.
4. Add the responsive header usage strip and details popover.
5. Keep terminal connection and telemetry reconnect state independent.
6. Update `docs/TERMINAL_PROTOCOL.md` or add a sibling telemetry protocol
   document.

### Phase 3: Metadata tree

1. Add paginated interaction summaries and detail loading.
2. Render interaction, plan, model, tool, event, and nested-agent nodes.
3. Support in-progress parents and out-of-order completion.
4. Add model, subagent, and tool breakdown filters.
5. Keep content capture false.

### Phase 4: Optional content tree

1. Add explicit durable content-capture configuration.
2. Add quotas, retention, deletion, redaction, and truncation.
3. Parse stringified message, tool, and system-instruction structures.
4. Render user messages, final responses, arguments, and results safely.
5. Validate storage growth with long real sessions before enabling the option.

## Validation Plan

### Parser fixtures

Include sanitized fixtures for:

- one model call with cache creation;
- a follow-up call with cache read and incremental cache write;
- a provider with missing cache fields;
- reasoning tokens;
- multiple model calls under one interaction;
- tool success and tool failure;
- nested subagent invocation using another model;
- recursive or concurrent subagents;
- plan mode;
- out-of-order child and parent lines;
- duplicate spans and repeated cumulative metrics;
- malformed JSON and a partial final line;
- unknown attributes, span kinds, and events;
- process restart with multiple JSONL files; and
- content-enabled stringified fields and truncation.

### Accounting invariants

Assert:

- root and child aggregates are never both counted;
- session total equals the sum of unique `chat` spans;
- subagent usage is a subset of session usage;
- input categories never increase total input;
- reasoning is not double-counted;
- cache percentage uses token-weighted totals;
- missing cache reporting remains nullable;
- metric snapshots do not inflate totals; and
- nano-AIU aggregation uses integer arithmetic.

### Runtime integration

Using a real container and Copilot CLI:

1. start a CLI session and observe telemetry before the process exits;
2. send multiple user interactions and verify separate top-level interaction
   trees within the same Copilot conversation;
3. run a tool and verify metadata-only fields;
4. invoke a subagent and verify parentage and token attribution;
5. stop and resume the pane and verify prior files remain visible;
6. restart `kernel_host`, `agent_host`, and `client_service` independently and
   verify replay;
7. destroy and recreate the kernel container with the same durable volume;
8. attach two browsers and verify no duplicate accounting;
9. disconnect telemetry while keeping the terminal usable; and
10. delete the session and verify telemetry-volume cleanup.

### Content integration

Before Phase 4:

- prove capture false omits messages, arguments, and results;
- prove capture true includes them;
- measure representative file growth;
- inject HTML, ANSI, control characters, and large payloads;
- verify public projection and escaping;
- verify secrets and `enduser.pseudo.id` are not exposed; and
- verify deleting telemetry removes raw and normalized content.

### UI validation

- unit-test formatting, null states, coverage, and stale state;
- test narrow and wide layouts;
- verify terminal rows remain fully visible;
- verify the details popover does not resize or cover xterm unexpectedly;
- run `just webui-screenshots` in light and dark themes; and
- inspect active-session screenshots using
  `.claude/skills/validate-webui-screenshots/SKILL.md`.

Finish implementation with `just check` and the real restart/adoption test
required by `.claude/skills/evolve-durable-sessions/SKILL.md`.

## Required File Areas

Likely implementation surfaces:

- `kernels/kernel_host/src/kernel_host/terminal.py`
  - reserve telemetry environment and create per-launch paths;
- new kernel telemetry modules
  - provider protocol, JSONL reader, normalized models, and Copilot adapter;
- `kernels/kernel_host/src/kernel_host/app.py`
  - internal snapshot, page, and stream routes;
- `services/agent_host_rs/src/docker_runtime.rs`
  - telemetry volume lifecycle and mount;
- `services/agent_host_rs/src/models.rs`
  - runtime identity and cleanup resource kinds;
- `services/agent_host_rs/src/sessions.rs`
  - internal telemetry methods;
- `services/agent_host_rs/src/terminal.rs` or a sibling telemetry module
  - internal routes and bounded proxying;
- `services/client_service_rs/src/models.rs`
  - durable volume identity and public normalized types;
- `services/client_service_rs/src/store/sqlite.rs`
  - backward-compatible durable-session migration if identity is persisted;
- `services/client_service_rs/src/agent_host.rs`
  - snapshot/page/stream client methods;
- `services/client_service_rs/src/api.rs`
  - public typed routes and mode checks;
- `clients/webui/src/types.ts`
  - normalized telemetry types;
- `clients/webui/src/api.ts`
  - summary, page, and stream clients;
- `clients/webui/src/queries.ts`
  - telemetry cache and fallback refresh;
- `clients/webui/src/CliView.tsx`
  - header summary, details, and future tree launcher;
- `clients/webui/src/cli-view.css`
  - responsive summary layout without touching terminal sizing; and
- `docs/TERMINAL_PROTOCOL.md` or a new telemetry protocol document
  - public contract and failure behavior.

## Open Validation Items

These do not block the architecture, but must be settled with fixtures before
their related UI ships:

1. Confirm exact multi-user-interaction trace behavior in a long-lived
   interactive `1.0.81-0` process.
2. Capture plan mode and document its actual intermediate span attributes.
3. Capture failed tools and failed model calls.
4. Verify cache field behavior for every supported BYOK API flavor and model.
5. Verify explicit compaction and truncation event names, if any, in current
   OTel output.
6. Decide whether partial cache reporting should be hidden or shown with a
   coverage badge.
7. Measure long-session JSONL growth with content capture disabled.
8. Determine raw telemetry retention before allowing content capture.

## References

- Installed CLI: `copilot help monitoring`
- GitHub Copilot CLI command reference:
  <https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference>
- OpenTelemetry GenAI semantic conventions:
  <https://opentelemetry.io/docs/specs/semconv/gen-ai/>
- Copilot SDK OpenTelemetry guide:
  <https://github.com/github/copilot-sdk/blob/main/docs/observability/opentelemetry.md>
- Community parser used as corroboration, not as the AgentSpace abstraction:
  <https://github.com/ccusage/ccusage/tree/main/rust/adapters/copilot>

## Reviewer Feedback

Peer review of the plan as written. No content above this section was changed.
Findings were checked against the current repository (`kernels/copilot_launch`,
`kernels/kernel_host`, `services/agent_host_rs`, `services/client_service_rs`,
`clients/webui`) and against the installed `copilot help monitoring` output.

### Overall assessment

The plan is unusually careful in the right places: nullable-instead-of-zero
accounting, refusing to fabricate a system-prompt token count, labeling
cache-break detection as inference, keeping Copilot attribute names below a
harness boundary, and treating telemetry as auxiliary to the PTY path. The
volume model matches the existing `agentspace-session-workspace-*` pattern and
the `CleanupResourceKind` machinery in `docker_runtime.rs`, and the durable
field proposal matches `.claude/skills/evolve-durable-sessions/SKILL.md`.

The problems below are mostly about (a) one concrete accounting bug, (b) the
durability story for raw JSONL, and (c) the security/reserved-environment
surface, which is larger than the plan assumes.

### Blocking correctness issues

1. **The deduplication key is wrong and can double-count.** "Deduplicate spans
   by `(source file identity, traceId, spanId)`" makes the file part of the
   identity, so the exact scenario the plan anticipates — the same span present
   in two files, which the plan itself observed when two processes shared one
   path — will *not* deduplicate. The key must be `(traceId, spanId)`, with
   file identity and end time retained only as conflict diagnostics. Keep the
   response-ID check as a secondary guard, not the primary one.

2. **The v1 summary depends on the least-verified grouping rule.** "Last
   interaction" requires one top-level `invoke_agent` per user interaction, but
   Open Validation Item 1 admits multi-interaction trace behavior in a
   long-lived interactive process is unconfirmed. Define the grouping algorithm
   now with an explicit precedence — `github.copilot.interaction_id` first,
   root-`invoke_agent` span tree second, trace ID last — or ship v1 with
   "session totals + last model call" (both unambiguously available) and gate
   the "Last interaction" row behind the fixture. As written, a wrong guess
   silently produces a plausible but incorrect headline number.

3. **Cache-inclusive versus cache-additive reporting is treated as malformed
   data.** The plan derives `uncached_input = input - cache_read - cache_write`
   and clamps negatives to unknown with a parser warning. That is correct for
   OpenAI-style reporting (which the local GPT capture confirms), but
   Anthropic-style APIs report input tokens *excluding* cache reads and cache
   creation, so a healthy BYOK provider would emit a permanent stream of parser
   warnings and a null cache percentage. Introduce an explicit per-call
   convention detection and an `effective_input_tokens` value:

   - if `cache_read + cache_write <= input`, treat reporting as inclusive;
   - otherwise treat it as additive and use
     `effective_input = input + cache_read + cache_write`.

   Compute both `cache_reuse_percent` and session totals from
   `effective_input_tokens`, record the detected convention on the call, and
   reserve the warning path for values that fit neither shape. Mixing
   conventions inside one session total is otherwise silently wrong.

4. **Ordering by end time will show the wrong "Last" value.** "Last-call
   totals: use the newest completed `chat` span by end time" and the
   consecutive-call cache-lane comparison both use completion order, yet the
   plan elsewhere states correctly that completion order is not tree order and
   that background subagents finish out of order. A long-running subagent model
   call that ends after the foreground call would hijack the summary. Order by
   `startTime` for "last" selection and for cache-lane adjacency; keep end time
   only for "is it complete".

### Durability and lifecycle gaps

5. **Raw JSONL as the sole replay log conflicts with the plan's own bounds.**
   The plan declares the raw file the replay log, then also requires bounding
   "total indexed nodes" and defers a retention policy to Phase 4. Those cannot
   both hold: once the index bound is hit, the in-memory view diverges from the
   file, and after a restart the replay may drop a different set of nodes, so
   the same session shows different totals before and after a kernel restart.
   Recommend adding, in Phase 1, a small normalized checkpoint written by
   `kernel_host` into the same volume (running aggregates plus per-file byte
   offsets plus a bounded interaction index). That makes restart replay O(new
   bytes) instead of O(session), makes totals stable under index eviction, and
   allows raw files to be rotated or truncated later without losing the summary
   that the v1 UI actually shows.

6. **Telemetry is unreadable whenever the kernel container is not running.**
   The kernel container is per-session and is removed on stop/cleanup, and only
   it mounts the telemetry volume, so a stopped session — or any freshly opened
   browser pointed at one — shows nothing. The plan's answer ("the WebUI retains
   its last snapshot as stale") only covers a browser that was already
   connected. Since the whole point is post-hoc usage inspection, persist a
   small last-known `UsageBreakdown` (plus `updated_at`) in the durable client
   session alongside `telemetry_volume_identity`, or have `agent_host` read the
   volume through a short-lived helper container. Note this also raises the
   question of *when* the snapshot is persisted; on every coalesced update is
   probably too often, on session stop is probably too late if the container
   dies unexpectedly.

7. **Unflushed spans at process death are not addressed.** Pane stop, container
   removal, and `SIGKILL` will all leave the last spans unexported and possibly
   a partial final line permanently unterminated. The reader is told to "wait
   for completion" of a partial line, which would wait forever. Add an explicit
   rule: a partial trailing line in a file whose owning launch is known dead is
   discarded with a counted warning, not held open.

8. **Chat mode inherits the same environment builder.** `build_copilot_env`
   in `kernels/copilot_launch` is shared by `build_interactive_launch`
   (terminal) and `build_chat_launch` (`kernel_copilot`, one Copilot process
   *per user message*). If telemetry env is injected there, a chat session
   produces one JSONL file per message and the directory grows to hundreds of
   files, which breaks the "discover and replay all `*.jsonl`" reader. Either
   gate injection on the CLI harness explicitly, or design the reader for a
   large file count with an index. The plan should state which, because the
   file-per-launch decision is otherwise fine for the terminal case only.

### Environment and security surface

9. **`kernels/copilot_launch` is missing from Required File Areas, and it is
   the correct enforcement point.** `build_copilot_environment` merges the
   process environment first and then `config_env`, so agent/connection values
   override anything the kernel sets. Reserved-key rejection belongs there,
   next to the existing `_RESERVED_SESSION_ARGS` validation, not in
   `terminal.py`.

10. **Reserve the namespace, not five keys.** The installed help documents
    `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`,
    `OTEL_EXPORTER_OTLP_PROTOCOL`, the certificate/mTLS variables,
    `OTEL_SERVICE_NAME`, `COPILOT_OTEL_SOURCE_NAME`, and `OTEL_LOG_LEVEL`. Two
    distinct risks follow:

    - *Exfiltration*: an endpoint plus headers could ship agent telemetry to a
      third party; combined with a content-capture flag this is a data-loss
      channel, and it is exactly the kind of thing an untrusted agent config
      should not be able to set.
    - *Silent breakage*: `COPILOT_OTEL_SOURCE_NAME` and `OTEL_SERVICE_NAME`
      change the scope/resource fields the adapter keys on.

    Reject or overwrite the whole `OTEL_*` and `COPILOT_OTEL_*` prefix space
    with a documented allowlist, so a future CLI variable is denied by default.

11. **Make `COPILOT_OTEL_EXPORTER_TYPE=file` a validated invariant, and note
    why.** The help states `COPILOT_OTEL_FILE_EXPORTER_PATH` selects the file
    exporter *only when* `COPILOT_OTEL_EXPORTER_TYPE` is unset. The plan sets
    the type explicitly, which is right, but the reasoning given ("makes intent
    unambiguous") understates it: a stray `otlp-http` value would silently
    disable local capture. Assert the resolved value at launch.

12. **Enterprise managed telemetry is unaddressed.** The same help text says
    export can also be enabled by "enterprise managed telemetry settings". Add
    an open item: determine whether managed settings can override exporter type
    or force a second exporter, and what AgentSpace does if an org policy
    enables content capture that v1 promises is off.

13. **State the trust boundary honestly.** The telemetry directory is mounted
    read-write into the container that also runs the agent's own shell, so the
    agent can read, corrupt, delete, or forge its own telemetry, and can write
    arbitrary bytes that `kernel_host` then parses. This does not invalidate the
    design — the workspace has the same property — but the plan currently
    describes the volume as "private to the kernel runtime", which reads
    stronger than it is. Say plainly that telemetry is not tamper-evident with
    respect to the agent, and that line-size/record bounds are a security
    control rather than only a robustness one.

14. **Move the "capture false omits content" proof into Phase 1.** It is
    currently listed under "Content integration / Before Phase 4", but v1 ships
    with capture false *and durably stores the raw file*. The assertion that
    disabled capture emits no prompts, arguments, results, or tool definitions
    is a v1 privacy guarantee and should be a Phase 1 fixture test, together
    with the `enduser.pseudo.id` discard test.

### Design and scope suggestions

15. **Consider dropping the NDJSON stream from v1.** Spans are exported on span
    end, so the data is inherently chunky; a `GET /sessions/{id}/telemetry`
    polled by React Query every two to five seconds while the CLI view is
    focused would deliver nearly identical perceived latency for a five-line
    summary, with none of the stream fan-out, backpressure, reconnect, or
    proxy work across three services. A "Phase 0 walking skeleton" — env plus
    volume plus one polled summary endpoint plus the header strip — would
    validate the whole pipeline in a fraction of the surface, and the stream
    could arrive with the tree in Phase 3 when it is actually justified.

16. **Record the local HTTPS OTLP collector as a considered alternative.** The
    plan dismisses OTLP because cleartext is refused, but the help documents
    `OTEL_EXPORTER_OTLP_CERTIFICATE` for a private CA, so a collector in or
    beside `agent_host` with a generated certificate is feasible. It would
    solve findings 6 and 13 (no live kernel required, agent cannot tamper with
    already-pushed data). It is correctly rejected for v1 on cost grounds, but
    it should be documented as the likely v2 direction rather than left looking
    impossible.

17. **Resolve the "unavailable versus rejected" inconsistency.** The plan says
    unsupported harnesses return a typed `unavailable` capability, and also
    says the public route should "apply the same mode and ownership validation
    as terminal routes" — but the existing terminal routes reject non-CLI
    sessions outright. Pick one; `unavailable` is the better fit for a UI that
    may render the strip in more than one view.

18. **Apply the aggregate double-count rule to cost as well as tokens.** The
    "do not add `invoke_agent` usage to child `chat` usage" rule is stated only
    for tokens, but `github.copilot.cost` and `github.copilot.nano_aiu` appear
    on agent spans too. Say explicitly that credits and cost are summed from
    `chat` spans only, and add it to the accounting invariants list.

19. **Reconsider the `uncached_input_tokens` label.** Cache-*write* tokens were
    also processed fresh by the provider; only cache-*read* tokens were cheap.
    A user reading "uncached input: 3" next to "cache write: 58" may conclude
    only three tokens were billed at full rate. Either rename the field (for
    example `fresh_input_tokens` = non-cache-read input) or make the details
    popover show read / write / neither as three explicit, summing categories.

20. **Label context occupancy as of its source event.** `Context 17.8k / 272k`
    comes from a `github.copilot.session.usage_info` event attached to a model
    span, so it is "as of the last model call", not "now" — and it will be
    conspicuously stale right after a compaction until the next model call. The
    tooltip should say so; otherwise this is the value users will most quickly
    call a bug.

21. **Terminology nit in the aggregation rules.** "Sum leaf `chat` spans under
    the top-level `invoke_agent`" should read "descendant `chat` spans": `chat`
    spans are not necessarily leaves, and the surrounding text elsewhere
    correctly says descendants. Similarly, `gen_ai.client.token.usage` is
    described in the CLI help as a histogram; calling it "cumulative" muddles
    an otherwise correct argument for preferring spans.

22. **Documentation targets are slightly under-scoped.** Given the deliberate
    decision to keep the route out of `terminal/`, a new
    `docs/TELEMETRY_PROTOCOL.md` is clearly preferable to extending
    `docs/TERMINAL_PROTOCOL.md`. `docs/OPERATIONS.md` also needs an edit: it
    enumerates the managed volume kinds and cleanup expectations, and a new
    labeled per-session volume that operators may encounter belongs in that
    inventory.

### Suggested additional open validation items

- Exporter flush behavior on pane stop, container removal, and `SIGKILL`,
  including whether the final record is reliably newline-terminated.
- File permissions and umask of the exporter-created JSONL inside the
  container, since the plan asserts private permissions but does not verify who
  creates the file.
- Timestamp skew between the container clock and the service clock, because
  snapshot "age"/staleness in the UI is computed across that boundary.
- Behavior when a pane is respawned and two Copilot generations briefly overlap
  in the same directory.
- Whether `gen_ai.conversation.id` remains stable across `--resume`, and what
  "session totals" should mean when it does not.
- Growth in *file count* (not only bytes) for chat-mode sessions if telemetry is
  not gated to the CLI harness.

## Response to Feedback

The review identified several correctness and lifecycle issues that should
change the implementation. The decisions below supersede conflicting details
in the plan above; the original plan and review remain unchanged for traceability.

### Accepted correctness changes

1. **Span identity will not include file identity.** The primary key will be
   `(trace_id, span_id)`. Source file, byte offset, end time, response ID, and a
   normalized payload hash will be retained as provenance and conflict
   diagnostics. Response ID remains a secondary duplicate guard.

   The review's conclusion is correct even though the referenced local append
   probe did not itself duplicate a span: that probe appended records from two
   separate Copilot processes. Including file identity would still make true
   cross-file duplicates impossible to recognize.

2. **"Last interaction" will be gated by a long-lived interactive fixture.**
   V1 may show session totals and the latest comparable completed model call,
   but it will not label a value "Last interaction" until multiple user
   messages in one resumed interactive process prove the grouping behavior.

   The proposed precedence of interaction ID before span hierarchy will not be
   adopted literally. In the captured subagent trace, subagent model calls did
   not carry the top-level `github.copilot.interaction_id`; they were connected
   through parent spans. The grouping rule will therefore be:

   - identify a top-level `invoke_agent` root and its trace/tree;
   - use `github.copilot.interaction_id` to corroborate and label model calls
     where present;
   - include descendants through span parentage, including subagents without
     the interaction attribute; and
   - fall back to a trace-scoped unknown interaction only when the root is
     absent.

   A plausible partial grouping must never be presented as a complete
   interaction total.

3. **Token accounting will support inclusive and additive cache conventions.**
   The review is correct that provider APIs can report cache categories
   differently. Anthropic Messages usage is additive, while the locally
   captured Copilot/GitHub records were inclusive.

   Add these normalized fields:

   ```text
   token_accounting_convention: inclusive | additive | unknown
   effective_input_tokens
   cache_read_input_tokens
   cache_write_input_tokens
   other_input_tokens
   fresh_input_tokens
   ```

   For inclusive records:

   ```text
   effective_input = input
   other_input = input - cache_read - cache_write
   fresh_input = cache_write + other_input
   ```

   For additive records:

   ```text
   effective_input = input + cache_read + cache_write
   other_input = input
   fresh_input = input + cache_write
   ```

   Total tokens and cache reuse use `effective_input_tokens`.

   The suggested shape test is only partially sufficient:
   `cache_read + cache_write > input` proves the record cannot be inclusive,
   but the inverse does not prove that it is inclusive. Convention resolution
   will use, in order:

   - a fixture-verified adapter rule for the configured provider/wire API;
   - explicit source metadata if Copilot adds it;
   - a conclusive numeric shape; and
   - `unknown` when the remaining shape is ambiguous.

   Ambiguous calls remain visible but are excluded from convention-dependent
   percentages and carry reporting coverage.

4. **Call selection and cache adjacency will use start order, not completion
   order.** Only completed spans contribute usage, but "latest" selection and
   consecutive-lane comparisons use `startTime`. Foreground summaries exclude
   calls nested under subagents unless explicitly labeled as subagent activity.
   `endTime` determines completion and duration only.

5. **"Leaf chat span" will be replaced with "descendant chat span."** Exact
   totals sum unique descendant `chat` spans. The implementation will not
   assume that a chat span is structurally a leaf.

6. **Costs and AI credits follow the same double-count rule as tokens.**
   `github.copilot.cost` and `github.copilot.nano_aiu` will be summed from
   unique `chat` spans only. Aggregate `invoke_agent` values are reconciliation
   data, not additional usage.

7. **The UI will use explicit input categories.** The misleading
   `uncached_input_tokens` label will be dropped. Details will show:

   - cache read;
   - cache write;
   - other input; and
   - fresh input, defined as all effective input not served from cache.

   This makes clear that cache-write tokens were processed fresh and may have a
   distinct price.

8. **Context occupancy will carry its observation time.** The model will
   include `context_observed_at`, and the UI will say "as of the last model
   call." It will not imply that the value changes continuously or immediately
   after compaction.

9. **Metrics wording will be corrected.** `gen_ai.client.token.usage` is an
   OTel histogram and may be exported as repeated snapshots. The reason to
   prefer spans is exact per-call attribution and avoiding repeated aggregate
   observations, not that the metric itself should generically be called
   cumulative.

### Accepted durability and lifecycle changes

10. **Phase 1 will write a normalized checkpoint into the telemetry volume.**
    Raw JSONL remains the audit/replay source, but it will no longer be the only
    durable state needed to produce stable totals.

    The atomic, versioned checkpoint will contain:

    - aggregate usage and activity counts;
    - accounting/reporting coverage;
    - processed file identities and byte offsets;
    - known `(trace_id, span_id)` identities or a versioned compact
      representation sufficient for replay;
    - the bounded latest-interaction index; and
    - parser warnings and source-version metadata.

    On restart, `kernel_host` loads the checkpoint and processes only complete
    records after the saved offsets. Tree-node eviction cannot change session
    totals. Checkpoint format changes require explicit migration or a full raw
    replay. Checkpoint replacement must use write, flush, and atomic rename in
    the same volume.

11. **Dead-launch partial lines will be discarded explicitly.** A trailing
    partial line is retained only while its launch may still write. Once the
    owning Copilot process is known dead, the reader discards it, records file
    identity and byte count in a warning, advances the checkpoint safely, and
    never waits forever.

12. **Telemetry injection is CLI-only in this implementation.** Shared Copilot
    provider/session semantics remain in `kernels/copilot_launch`, but the
    managed telemetry policy is passed only by
    `build_interactive_launch`. `build_chat_launch` must not inherit a
    per-process JSONL path. Chat telemetry can adopt the normalized contract
    later with a lifecycle and storage strategy appropriate to one process per
    message.

13. **No duplicate summary will be added to `client_service` SQLite in v1.**
    The review's premise that **Stop CLI** removes the kernel container is not
    correct: the current stop path calls tmux `kill-session`; the
    `kernel_host` container remains available. Browser selection also ensures
    or recovers the runtime before showing the active CLI.

    The normalized checkpoint plus durable telemetry volume addresses replay
    after container recreation. When the kernel is unexpectedly unavailable,
    the WebUI may retain its in-memory value as stale; a newly opened client
    receives `unavailable` until runtime recovery mounts and replays the
    volume. This is preferable to maintaining a second partially synchronized
    aggregate in the client-session row. A helper container or central
    collector remains an option if offline inspection becomes a requirement.

### Accepted environment and trust-boundary changes

14. **`kernels/copilot_launch` is the enforcement point and will be added to
    the implementation file list.** The current environment builder copies
    process environment and then applies configured environment, so enforcement
    after that merge is required.

    The launcher will distinguish untrusted configured values from
    AgentSpace-owned managed values:

    1. build the normal Copilot environment;
    2. remove all inherited or configured `OTEL_*` and
       `COPILOT_OTEL_*` values;
    3. add the exact AgentSpace-managed allowlist; and
    4. assert the resolved values before exec.

    Agent and connection configuration using either reserved prefix will be
    rejected with an exact field path rather than silently ignored.

15. **Exporter type is a validated invariant.** The resolved environment must
    contain:

    ```text
    COPILOT_OTEL_EXPORTER_TYPE=file
    ```

    together with the AgentSpace-owned file path and content-capture policy.
    This is required for correctness, not merely clarity: an inherited
    `otlp-http` value can prevent the file path from selecting the local
    exporter.

16. **Enterprise-managed telemetry is a Phase 1 validation gate.** Official
    managed settings have higher precedence than user settings, and current
    Copilot help states that managed telemetry can enable export. Before
    claiming that v1 guarantees content capture is off, implementation must
    determine whether policy can override exporter type, file path, or
    `captureContent`.

    If managed policy defeats the local file exporter, telemetry reports a
    typed policy conflict rather than empty success. If content-bearing fields
    appear while AgentSpace requested metadata-only capture, the adapter must
    exclude them from public projection, mark a policy violation, and stop
    claiming metadata-only storage. The raw-file privacy consequence must be
    resolved before release; it cannot be fixed merely by redacting the API.

17. **The telemetry volume is not tamper-evident.** "Private" means isolated
    from other AgentSpace sessions and excluded from workspace snapshots, not
    protected from the agent inside its own container. The same agent shell can
    read, delete, corrupt, or forge its telemetry.

    JSON line-size limits, schema validation, file-count limits, path
    confinement, record-count bounds, and fail-safe parser behavior are
    security controls against this trust boundary. UI copy must describe the
    data as agent-reported usage, not an audit-grade billing record.

18. **Metadata-only privacy tests move to Phase 1.** Fixtures and a real probe
    must prove that capture false omits prompt text, response text, tool
    arguments, and tool results, and that `enduser.pseudo.id` never reaches the
    normalized/public model.

    The review's stronger suggestion that tool definitions should be absent is
    not adopted. The local `1.0.81-0` capture with content disabled still
    emitted a compact `gen_ai.tool.definitions` inventory containing tool type
    and name. That metadata is allowed, bounded, and not treated as tool
    content; full descriptions and schemas remain forbidden in metadata mode.

19. **File ownership and permissions become a Phase 1 test, not an
    assumption.** Verify directory mode, exporter-created file mode, process
    UID/GID, and umask in the real kernel image. Documentation will state the
    observed guarantee rather than promising stronger isolation.

### Accepted scope and API changes

20. **V1 will poll a summary endpoint instead of streaming it.** The first
    walking skeleton is:

    ```text
    managed env + telemetry volume + parser/checkpoint
      -> GET /sessions/{id}/telemetry
      -> focused React Query polling
      -> CLI header strip
    ```

    Poll every two seconds while CLI View is focused and a turn is active, and
    every five seconds while selected but idle. Suspend polling when the view
    is hidden. Span-end export is already chunky, so an NDJSON stream offers
    little additional perceived freshness for the summary.

    The stream is deferred to the metadata-tree phase, where incremental nodes
    justify fan-out and reconnect complexity. This also removes stream
    backpressure and proxy work from the initial implementation.

21. **The telemetry route returns typed `unavailable` for unsupported session
    modes or harnesses.** An unknown session remains `404`; an existing session
    without a telemetry provider returns a valid capability snapshot with
    `state=unavailable` and a machine-readable reason. It does not inherit
    terminal routes' CLI-only `409` behavior.

22. **Documentation will use a dedicated telemetry protocol.** Add
    `docs/TELEMETRY_PROTOCOL.md` for normalized models, routes, bounds,
    lifecycle, and failure semantics. Update `docs/OPERATIONS.md` with the
    telemetry volume, backup expectations, cleanup role, storage growth, and
    tamper limitations. `docs/TERMINAL_PROTOCOL.md` only needs a cross-reference
    confirming that telemetry is not carried in PTY frames.

23. **A local HTTPS OTLP collector is recorded as a deferred alternative.**
    Copilot supports private-CA trust through
    `OTEL_EXPORTER_OTLP_CERTIFICATE`, so OTLP is technically feasible. It is
    rejected for v1 because it adds certificate generation/distribution,
    collector lifecycle, buffering, and another service to a trusted personal
    deployment whose user explicitly does not want certificate setup.

    Reconsider it if requirements expand to tamper resistance, offline querying
    without runtime recovery, centralized multi-harness collection, or external
    observability export. Certificates would need to be generated and managed
    automatically; no manual host certificate installation should be required.

### Additional validation accepted

The implementation validation list will also include:

- exporter flush and final-newline behavior on normal pane stop, container
  removal, and `SIGKILL`;
- timestamp skew between container and service clocks, with age computed from
  receipt time when source-clock trust is insufficient;
- overlapping Copilot generations during pane respawn;
- stability of `gen_ai.conversation.id` across `--resume`;
- the definition of session totals if a resumed process reports another
  conversation ID;
- explicit proof that Chat launch does not receive managed telemetry variables;
- file-count and per-file-size quotas, checkpoint behavior, and rotation;
- failure and recovery when the checkpoint is corrupt or newer than the reader;
- deterministic totals before and after index eviction and kernel restart; and
- cleanup/adoption tests covering the new telemetry volume role.

### Advice not adopted

Two recommendations are not adopted as written:

1. **Persisting `UsageBreakdown` in the client-session row.** Current terminal
   stop does not remove the kernel container, and runtime recovery already
   remounts durable volumes. A normalized checkpoint in that volume provides
   stable totals without introducing dual-write consistency between Python
   ingestion and Rust SQLite. Offline telemetry while intentionally avoiding
   runtime recovery is not a v1 requirement.

2. **Inferring inclusive accounting whenever cache categories fit inside
   `input_tokens`.** That shape is consistent with inclusive reporting but does
   not prove it; an additive record can coincidentally have more ordinary input
   than cache input. Provider/wire fixtures or explicit metadata must resolve
   the convention, otherwise the convention-dependent aggregate remains
   unknown.

With these changes, implementation should begin with corrected accounting,
environment enforcement, durable checkpointing, and one polled summary route.
The tree and content-capture features remain later phases.
