# Copilot CLI Telemetry Implementation Plan

## Status

Final implementation plan.

This plan is based on:

- GitHub Copilot CLI `1.0.81-0`;
- the installed `copilot help monitoring` documentation;
- the OpenTelemetry GenAI semantic conventions;
- local metadata-only, content-enabled, tool, subagent, live-write, and
  append-behavior captures made on 2026-08-15; and
- review of the current AgentSpace runtime, persistence, terminal, and WebUI
  boundaries.

## Goal

Collect structured, near-real-time information from an interactive Copilot CLI
session without reading or interpreting terminal output.

The first release will add a compact telemetry summary to CLI View showing:

- whole-session token usage;
- latest comparable model-call usage;
- effective input and output tokens;
- cache-read and cache-write tokens;
- cache reuse percentage and reporting coverage;
- model-call, tool-call, and subagent counts;
- current context occupancy as of the last reported model call;
- AI-credit usage when reported; and
- telemetry health and freshness.

The architecture must also support a later, dismissible tree view containing:

- one top-level node per user interaction;
- model calls made while producing the response;
- tools in execution order;
- nested subagent invocations and their own model/tool activity;
- plans and other intermediate spans;
- optional user messages and final responses; and
- optional tool arguments and results.

The summary and metadata tree must not require sensitive content capture.
Message and tool content are a later explicit opt-in.

## Non-Goals

- Screen scraping, ANSI parsing, or deriving state from xterm/tmux output.
- Redirecting or parsing Copilot stdout, stderr, or human-oriented debug logs.
- Mixing telemetry into terminal WebSocket frames.
- Claiming an exact system-prompt token count when Copilot does not report one.
- Claiming a prompt/KV-cache break is certain when it is inferred.
- Capturing prompts, source code, shell output, or tool payloads by default.
- Treating agent-produced telemetry as tamper-evident billing evidence.
- Adding telemetry to Chat mode in the first release.
- Running an OTLP collector or requiring certificate setup in the first
  release.
- Exposing Copilot-specific OTel attributes above `kernel_host`.

## Final Decisions

1. Use Copilot's supported OpenTelemetry JSONL file exporter.
2. Enable it only for interactive Copilot CLI launches in v1.
3. Force metadata-only capture and validate that policy before release.
4. Store source files in a labeled, per-session telemetry volume.
5. Persist the telemetry-volume identity in the durable session.
6. Maintain a versioned normalized checkpoint in the same volume.
7. Parse and normalize Copilot records inside `kernel_host`.
8. Use unique completed `chat` spans as the authoritative usage records.
9. Deduplicate spans by `(trace_id, span_id)`, never by source file.
10. Support both cache-inclusive and cache-additive token conventions.
11. Use span parentage and start order, not JSONL or completion order, to
    reconstruct activity.
12. Expose nullable values and coverage; never turn unknown into zero.
13. Label the cache metric "cache reuse," not request hit rate.
14. Treat cache-reset detection as an inference with confidence and reason.
15. Do not show system-prompt tokens; show observed context occupancy instead.
16. Poll one summary endpoint in v1; defer a realtime event stream to the tree.
17. Keep telemetry availability separate from terminal availability.
18. Return typed `unavailable` for unsupported sessions or harnesses.
19. Do not duplicate usage summaries into `client_service` SQLite in v1.
20. Keep prompts, responses, tool arguments, and tool results out of v1.

## Verified Copilot Behavior

### Activation

Copilot OTel activates when any of these are present:

```text
COPILOT_OTEL_ENABLED=true
OTEL_EXPORTER_OTLP_ENDPOINT=<url>
COPILOT_OTEL_FILE_EXPORTER_PATH=<path>
```

Supported exporter types are `otlp-http` and `file`. The file exporter writes
JSON Lines and does not require a collector or certificates.

### Record envelope

The source file contains one JSON object per line. Useful record types are:

- `type: "span"` for agent, model, plan, and tool activity; and
- `type: "metric"` for OTel metric observations.

Observed span fields include:

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

Timestamps are `[unix_seconds, nanoseconds]` pairs. Children often appear in
the file before parents because spans are exported when they end. File order is
completion/export order, not execution or tree order.

### Ordinary and subagent hierarchy

An ordinary interaction is:

```text
invoke_agent
  chat <model>
  execute_tool <tool>
  chat <model>
  ...
```

A locally captured subagent invocation on `1.0.81-0` was:

```text
invoke_agent
  chat gpt-5.6-sol
  execute_tool task
    invoke_agent task
      chat claude-haiku-4.5
  chat gpt-5.6-sol
```

Every node shared one trace ID. The nested agent reported:

```text
gen_ai.agent.id=builtin:task
gen_ai.agent.name=task
```

This is sufficient to count subagents, attribute descendant usage, and render
recursive agent activity.

Copilot monitoring help also documents:

```text
invoke_agent
  plan
    chat <model>
    execute_tool <tool>
```

Plan-mode output still requires a sanitized fixture before implementation.

### Live-write timing

A persistent interactive probe produced a completed `chat` span in the JSONL
file while Copilot remained alive, within five seconds of the call completing.
"Realtime" therefore means shortly after span completion, not token-by-token.

### Content-capture difference

With content capture disabled, a tool span exposed name, type, call ID, timing,
status, and trace relationships, but not arguments or results.

With:

```text
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true
```

the installed CLI added:

- `gen_ai.input.messages`;
- `gen_ai.output.messages`;
- `gen_ai.system_instructions`;
- full `gen_ai.tool.definitions`;
- `gen_ai.tool.description`;
- `gen_ai.tool.call.arguments`;
- `gen_ai.tool.call.result`; and
- tool-specific parameter attributes.

Logical arrays and objects were serialized as JSON strings. Consumers must
accept both stringified JSON and structured JSON values.

A minimal one-tool run grew from 18,337 bytes with content disabled to 262,782
bytes with content enabled for the same 16 records. Content capture is both a
privacy and storage multiplier.

Metadata-only output still included a compact tool inventory containing type
and name. That is allowed metadata; full tool descriptions and schemas are not.

## Source Identities and Ordering

Useful source identities include:

- `traceId`: the OTel trace containing an interaction tree;
- `spanId` and `parentSpanId`: tree identities and edges;
- `gen_ai.conversation.id`: Copilot's durable conversation/session identity;
- `github.copilot.interaction_id`: an interaction label on some model calls;
- `github.copilot.turn_id`: a model round within an interaction;
- `gen_ai.response.id`: a provider response identity;
- `gen_ai.request.previous_response.id`: a provider-dependent chain link; and
- `gen_ai.tool.call.id`: a tool-call identity.

`github.copilot.turn_id` is not a human-message number. One user interaction
can contain several model rounds.

Public terminology is:

- **interaction**: one user-to-agent operation;
- **model call** or **model round**: one completed `chat` span;
- **tool call**: one `execute_tool` span; and
- **subagent invocation**: a nested `invoke_agent` span.

### Deduplication

The unique span key is:

```text
(trace_id, span_id)
```

Source file, byte offset, end time, response ID, and a normalized payload hash
are provenance and conflict diagnostics, not identity.

If two records use the same span key:

- identical normalized records are one span;
- conflicting records retain the first complete record, mark telemetry
  `degraded`, and record both provenances; and
- response ID is a secondary model-call duplicate guard when present.

### Execution order

Only completed spans contribute usage. Among completed spans:

- execution and "latest" selection use `startTime`;
- `endTime` determines completion and duration;
- children attach through `parentSpanId`; and
- source-file order is ignored except for incremental reading.

This prevents a long-running background subagent that finishes late from
replacing a newer foreground call in the summary.

## Interaction Grouping

The intended grouping is:

1. identify a top-level `invoke_agent` root and its trace/tree;
2. include descendants through span parentage;
3. use `github.copilot.interaction_id` to corroborate and label model calls
   where present; and
4. retain a trace-scoped unknown interaction if the root is absent.

Interaction ID cannot be the sole grouping key because captured subagent calls
did not carry the top-level interaction ID; their relationship was expressed
through span parentage.

V1 will show **Latest call**, not **Last interaction**, until a long-lived
interactive fixture verifies that multiple user messages and resumed sessions
produce stable top-level grouping. A plausible partial tree must not be
presented as a complete interaction total.

## Token Accounting

### Source fields

Observed or supported attributes are:

| Meaning | Copilot attribute |
| --- | --- |
| Raw input | `gen_ai.usage.input_tokens` |
| Output | `gen_ai.usage.output_tokens` |
| Reasoning output | `gen_ai.usage.reasoning.output_tokens` |
| Cache read | `gen_ai.usage.cache_read.input_tokens` |
| Cache write | `gen_ai.usage.cache_creation.input_tokens` |

Also accept:

```text
gen_ai.usage.cache_write.input_tokens
```

as a cache-write alias.

Reasoning output is a provider-dependent detail/subset of output and must not
be added to output a second time.

### Inclusive and additive cache conventions

Provider APIs do not all define raw input the same way.

Normalize each model call to:

```text
token_accounting_convention: inclusive | additive | unknown
raw_input_tokens
effective_input_tokens
output_tokens
reasoning_output_tokens
cache_read_input_tokens
cache_write_input_tokens
other_input_tokens
fresh_input_tokens
total_tokens
```

For an inclusive record:

```text
effective_input = raw_input
other_input = raw_input - cache_read - cache_write
fresh_input = cache_write + other_input
total = effective_input + output
```

For an additive record:

```text
effective_input = raw_input + cache_read + cache_write
other_input = raw_input
fresh_input = raw_input + cache_write
total = effective_input + output
```

`fresh_input` means effective input not served from cache. It includes
cache-write tokens.

Resolve convention in this order:

1. a fixture-verified adapter rule for the configured provider/wire API;
2. explicit source metadata if Copilot adds it;
3. a conclusive numeric shape; and
4. `unknown`.

This numeric shape is conclusive only in one direction:

```text
cache_read + cache_write > raw_input
```

proves the record cannot be inclusive. The inverse does not prove inclusivity;
an additive record may contain more ordinary input than cache input.

Calls with unknown convention retain raw categories but do not contribute to
convention-dependent effective-input totals or cache percentages. Coverage
makes that omission explicit.

### Aggregation

Completed unique `chat` spans are authoritative:

- session totals sum every unique completed `chat` span;
- interaction totals sum descendant `chat` spans under its root;
- subagent totals sum descendant `chat` spans under nested agents;
- latest-call usage selects the latest-started comparable completed call; and
- model, agent, and provider breakdowns partition those same calls.

Do not add `invoke_agent` aggregates to child calls. That would double-count:

- tokens;
- opaque `github.copilot.cost`; and
- `github.copilot.nano_aiu`.

Agent-span aggregates are reconciliation data only.

OTel token metrics are histograms and may be emitted as repeated aggregate
observations. They are useful for diagnostics, but spans provide the exact
per-call attribution required here.

### Reporting coverage

Missing cache fields are not automatically zero.

Each model call reports:

```text
cache_reporting: reported | unreported
token_accounting_convention
```

If either cache attribute is present, the missing sibling can be treated as
zero for that call. If neither is present, cache semantics are unreported.

A snapshot includes:

```text
model_calls
cache_reported_calls
convention_resolved_calls
effective_input_covered_calls
```

Whole-session cache reuse is authoritative only when every included model call
has resolved cache reporting and accounting. Partial values may appear only in
expanded details with coverage labels.

### Cache reuse

For calls with resolved convention:

```text
cache_reuse_percent =
  sum(cache_read_input_tokens) / sum(effective_input_tokens) * 100
```

This is the share of effective input tokens served from cache. It is not the
percentage of requests containing a hit. Do not average per-call percentages.

### System prompt and context

Copilot does not report a separate exact system-prompt token count.

With content disabled, the system text is absent. With content enabled,
`gen_ai.system_instructions` contains text but not its provider tokenization.
Input also includes messages, tools, attachments, and other context.

The UI therefore shows "Not reported by Copilot" for system-prompt tokens.

When a `github.copilot.session.usage_info` event reports:

```text
github.copilot.current_tokens
github.copilot.token_limit
github.copilot.messages_length
```

normalize:

```text
context_tokens
context_limit
message_count
context_observed_at
```

The UI labels this "as of the last model call." It must not imply continuously
current occupancy, especially immediately after compaction.

## Prompt/KV-Cache Reset Detection

The tested JSONL did not emit an explicit `cache_break` event. Cache behavior
is observable; the cause of a reset is generally not.

Possible evidence includes:

- cache-read tokens collapse after high reuse;
- cache-write tokens rise to most effective input;
- context occupancy drops sharply;
- model identity changes;
- compaction or truncation events appear; or
- a provider response chain restarts.

A model change is an expected cache boundary, not a cache failure.

Expose raw cache values first. A derived signal is:

```text
state: healthy | cache_reset_suspected | expected_boundary | unknown
confidence: low | medium
reason:
  reuse_collapsed
  context_discontinuity
  compaction_or_truncation
  model_changed
```

Compare calls in start order on the same best-effort lane:

```text
conversation + nearest agent identity + model
```

Do not infer a reset for:

- a first call;
- a new subagent;
- a model change;
- unknown token convention;
- unreported cache semantics; or
- incomparable lanes.

Initial heuristic:

- prior comparable call reused at least 50% of effective input;
- current call reuses less than 10%; and
- current cache writes are at least 50% of effective input.

These are product thresholds, not protocol facts. Store raw values and compute
the signal in the adapter so thresholds can evolve without persistence
migrations.

If Copilot emits explicit compaction, truncation, or cache events, preserve
them and prefer explicit evidence. Do not parse Copilot's private session files
to invent stronger certainty.

## Subagents

A root `invoke_agent` without a parent is the top-level agent. A nested
`invoke_agent` is a subagent invocation.

Use these attributes when present:

```text
gen_ai.agent.id
gen_ai.agent.name
github.copilot.agent.type
```

Descendant model and tool spans belong to that agent. Names and IDs remain
optional.

Expose:

```text
subagent_invocations
subagent_model_calls
subagent_effective_input_tokens
subagent_output_tokens
subagent_cache_read_input_tokens
subagent_cache_write_input_tokens
subagent_duration_ms
```

Session totals already include subagent calls. Subagent totals are a breakdown,
not additional usage.

The parser must tolerate:

- recursively nested agents;
- concurrent/background agents;
- out-of-order completion;
- children exported before parents;
- subagents using other models;
- missing names/IDs; and
- an intermediate `execute_tool task` span.

Never infer execution from `github.copilot.context.custom_agent_names`; that is
an inventory of available agents.

## Tool Usage

### Metadata mode

An `execute_tool <tool>` span can provide:

- tool name;
- tool type;
- tool-call ID;
- trace and parent identities;
- start/end time and duration;
- status; and
- provider metadata.

This supports counts, duration, success/error state when reported, and a
metadata tree.

Metadata-only `gen_ai.tool.definitions` may expose bounded tool names/types.
Ignore full descriptions or schema-shaped values if they appear unexpectedly
and mark a content-policy violation.

Detailed error text is not guaranteed. Distinguish "error reported" from
"details unavailable."

### Future content mode

Content capture may contain:

- prompts and responses;
- system instructions;
- tool definitions;
- source/file contents;
- shell commands and output;
- arguments and results;
- paths and attachments; and
- secrets accidentally placed in context.

Copilot offers one boolean; it cannot capture messages but exclude tool
payloads at the source. Redacting after parsing does not remove plaintext from
the raw JSONL.

Content mode therefore requires:

- explicit per-agent or per-session opt-in;
- durable launch-snapshot intent;
- raw-file and field quotas;
- retention and deletion;
- truncation markers;
- safe UI escaping;
- validation of stringified JSON;
- a decision on encryption at rest; and
- separate privacy tests.

It is not part of v1.

## Other Available Information

### Model and request

- requested and response model;
- provider;
- response-chain IDs;
- finish reasons;
- streaming flag;
- service-request ID;
- server duration;
- time to first chunk; and
- output-chunk timing metrics.

Provider may remain `github` for several model families. Use model attributes
for model-family attribution.

### Cost and credits

- `github.copilot.cost` is opaque and must not be labeled dollars;
- `github.copilot.nano_aiu` is AI credits in nano-units; and
- display AI credits as nano-AIU divided by `1_000_000_000`.

Store nano-AIU with integer arithmetic where the source permits it.

### Activity and health

OTel metrics can report:

- model duration and tokens;
- time to first/output chunks;
- agent duration;
- inference and tool-call counts;
- tool duration;
- model rounds; and
- MCP connection attempts.

Spans remain authoritative for exact nodes and usage. Metrics are diagnostics
and cross-checks.

### Excluded source data

Discard during normalization:

- `enduser.pseudo.id`;
- raw resource attributes;
- instrumentation internals;
- arbitrary unknown attributes;
- raw service request IDs in ordinary UI;
- raw response IDs in ordinary UI; and
- content-bearing values in metadata mode.

Public APIs never return raw OTel records.

## Architecture

### Data flow

```text
Copilot CLI
  -> per-launch JSONL files
  -> per-session telemetry volume
  -> CopilotOtelTelemetryProvider in kernel_host
  -> normalized checkpoint + in-memory snapshot
  -> agent_host typed proxy
  -> client_service typed API
  -> focused WebUI polling
  -> CLI header summary
```

The PTY remains independent:

```text
Copilot CLI <-> tmux <-> terminal WebSocket <-> xterm
```

No PTY bytes enter the telemetry path.

### Harness boundary

Add a kernel-side protocol:

```python
class SessionTelemetryProvider(Protocol):
    async def snapshot(self) -> TelemetrySnapshot: ...
```

Later phases extend it with:

```python
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

- Copilot OTel environment variables;
- dotted attribute names;
- Copilot span/event names;
- cache aliases and accounting conventions;
- aggregate duplication;
- stringified content shapes; and
- Copilot-specific fallbacks.

Rust services and WebUI consume normalized models only. Another CLI can
implement the same provider contract with another source format.

### Managed launch environment

Interactive launch receives an AgentSpace-owned telemetry configuration,
separate from user/configured environment.

Resolved values are:

```text
COPILOT_OTEL_ENABLED=true
COPILOT_OTEL_EXPORTER_TYPE=file
COPILOT_OTEL_FILE_EXPORTER_PATH=/var/lib/agentspace/telemetry/<launch-id>.jsonl
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=false
OTEL_RESOURCE_ATTRIBUTES=agentspace.session.id=<durable-session-id>
```

`COPILOT_OTEL_EXPORTER_TYPE=file` is a validated invariant. Copilot selects the
file exporter from the path only when exporter type is unset; an inherited
`otlp-http` value could otherwise disable managed capture.

`kernels/copilot_launch` is the enforcement point because its environment
builder currently merges process environment and configured environment.

For interactive launch:

1. build the normal Copilot environment;
2. remove inherited/configured `OTEL_*` and `COPILOT_OTEL_*` values;
3. reject agent or connection configuration using those prefixes with an exact
   field path;
4. add only the AgentSpace-managed allowlist; and
5. assert the resolved policy before exec.

`build_chat_launch` receives no managed telemetry config. Tests must prove Chat
launch does not inherit these values.

### Enterprise-managed policy gate

Enterprise-managed Copilot settings have higher precedence than user settings,
and managed telemetry may enable or alter export.

Before v1 claims metadata-only storage, verify whether managed policy can
override:

- exporter type;
- file path;
- endpoint;
- content capture; or
- multiple exporters.

If managed policy defeats the file exporter, return a typed policy conflict.

If content-bearing attributes appear while AgentSpace requested metadata-only:

- exclude content from normalization/public projection;
- mark telemetry `degraded` with a policy-conflict reason;
- stop claiming metadata-only raw storage; and
- resolve the raw-file privacy consequence before release.

API redaction alone is not sufficient because source files already contain
plaintext.

### Per-session telemetry volume

Create:

```text
agentspace-session-telemetry-<stable-suffix>
  -> /var/lib/agentspace/telemetry
```

Do not use:

- `/root/.copilot`, which is shared across sessions;
- `/workspace`, which would enter workspace snapshots; or
- the container writable layer, which is lost on recreation.

Label the volume with:

```text
agentspace.managed=true
agentspace.role=session-telemetry
agentspace.session_id=<full durable session ID>
```

`agent_host` owns:

- deterministic naming;
- creation and adoption;
- full-label collision checks;
- mount validation;
- orphan review/cleanup;
- session deletion; and
- runtime recovery.

Persist `telemetry_volume_identity` in `SessionRecord`. Existing rows migrate
to `None`, explicitly meaning that managed telemetry history has not been
created. Do not infer recoverability from absence.

Terminal stop kills tmux but does not remove the kernel container. Unexpected
container loss is recovered through normal ensure/adoption, which remounts the
volume.

Do not persist `UsageBreakdown` in `client_service` SQLite in v1. The
kernel-side checkpoint is the single normalized durable summary and avoids a
cross-service dual-write protocol.

### Trust boundary

The volume is isolated from other AgentSpace sessions and workspace snapshots,
but it is read-write inside the same container as the agent shell.

The agent can:

- read telemetry;
- delete it;
- corrupt it;
- forge spans; or
- write hostile oversized records.

Telemetry is therefore agent-reported operational data, not tamper-evident
billing evidence.

Security controls include:

- confined managed paths;
- no user-derived filename components;
- line-size bounds;
- file-count and total-size quotas;
- JSON/schema validation;
- span/tree-depth bounds;
- normalized field bounds;
- parser error limits;
- no raw public endpoint; and
- fail-safe degradation without affecting tmux.

### Per-launch files

Generate one UUID-named JSONL file per interactive Copilot process launch.

Benefits:

- avoids depending on exporter truncate/append mode;
- isolates partial final records;
- makes pane respawn generations explicit; and
- preserves earlier launch data.

The terminal controller registers the active launch ID and path with the
telemetry provider. Pane exit/respawn marks the prior launch dead. After
recovery, identify any active launch through observed pane/process environment
or mark old launch files closed.

The reader must support many bounded files, but Chat mode is excluded from this
strategy.

### Incremental reader

The kernel provider:

- discovers managed `*.jsonl` files only;
- reads complete newline-terminated records;
- retains a partial tail only while its launch may write;
- discards a partial tail after the launch is known dead and records a warning;
- tracks file identity and byte offset;
- detects truncation/replacement;
- incrementally parses new records;
- attaches orphan children when parents arrive;
- tolerates unknown records;
- coalesces checkpoint writes;
- exposes parser health; and
- never stops Copilot on telemetry failure.

Use filesystem notifications when reliable with bounded polling fallback.

### Normalized checkpoint

Raw JSONL is the source evidence, but not the only state required for stable
totals.

Write a versioned checkpoint in the same volume containing:

- aggregate usage and activity counts;
- reporting/accounting coverage;
- processed file identities and byte offsets;
- known `(trace_id, span_id)` identities or an exact compact representation;
- bounded latest-interaction/model-call metadata;
- parser warnings;
- source versions; and
- checkpoint schema version.

Checkpoint replacement uses:

1. write a sibling temporary file;
2. flush and `fsync`;
3. atomic rename in the same filesystem; and
4. directory synchronization where supported.

On restart:

- validate checkpoint version and integrity;
- restore aggregates and dedup identities;
- continue after saved offsets;
- replay raw files from the beginning if the checkpoint is safely discardable;
- report `degraded` if a newer checkpoint cannot be understood; and
- never silently reset totals.

Tree-node eviction cannot change aggregate totals. Raw rotation may remove
records only after the checkpoint safely includes them.

## Normalized Public Model

### Snapshot

```text
TelemetrySnapshot
  schema_version
  state:
    starting | live | stale | unavailable | degraded
  reason: string | null
  content_mode:
    metadata | content | policy_conflict
  source_version: string | null
  observed_at: timestamp | null
  received_at: timestamp | null
  session: UsageBreakdown
  latest_call: ModelCallSummary | null
  last_interaction: UsageBreakdown | null
  context: ContextUsage | null
  counts: ActivityCounts
  subagents: SubagentBreakdown
  cache_signal: CacheSignal | null
  reporting: ReportingCoverage
  warnings: TelemetryWarningSummary
```

`last_interaction` remains null until grouping is fixture-verified.

### Usage breakdown

```text
UsageBreakdown
  raw_input_tokens: integer | null
  effective_input_tokens: integer | null
  output_tokens: integer | null
  total_tokens: integer | null
  reasoning_output_tokens: integer | null
  cache_read_input_tokens: integer | null
  cache_write_input_tokens: integer | null
  other_input_tokens: integer | null
  fresh_input_tokens: integer | null
  cache_reuse_percent: number | null
  nano_aiu: integer | null
  opaque_cost: number | null
```

Use optional values. A derived field is null when its prerequisites are
unknown.

### Activity counts

```text
ActivityCounts
  interactions
  model_calls
  tool_calls
  subagent_invocations
  subagent_model_calls
  errors
```

### Reporting coverage

```text
ReportingCoverage
  model_calls
  cache_reported_calls
  convention_resolved_calls
  effective_input_covered_calls
  context_reported
```

### Context

```text
ContextUsage
  tokens
  limit
  message_count
  observed_at
```

### Future tree node

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

Raw trace IDs may back opaque IDs but are not public domain semantics.

## API

### V1 internal routes

`kernel_host`:

```text
GET /telemetry
```

`agent_host`:

```text
GET /sessions/{runtime_session_id}/telemetry
```

`client_service`:

```text
GET /sessions/{session_id}/telemetry
```

All boundaries use typed normalized models.

### Availability semantics

- unknown durable session: `404`;
- existing session with no telemetry provider: `200`, `state=unavailable`;
- provider starting/no complete record yet: `200`, `state=starting`;
- valid snapshot: `200`, `state=live`;
- parser/schema/policy issue with usable partial data: `200`,
  `state=degraded`;
- runtime unavailable before recovery: `200`, `state=unavailable`;
- upstream transport failure: repository-standard `503`.

Do not return a zero-shaped successful snapshot for missing telemetry.

The telemetry route does not inherit terminal routes' CLI-only `409`; its
capability model is intended to support future Chat and other harnesses.

### Later tree routes

Phase 3 adds:

```text
GET /sessions/{session_id}/telemetry/interactions
GET /sessions/{session_id}/telemetry/interactions/{interaction_id}
GET /sessions/{session_id}/telemetry/stream
```

Interaction pages use bounded opaque cursors. The update stream uses NDJSON to
match existing infrastructure, starts with a snapshot/revision, and never
backpressures Copilot or tmux.

## WebUI

### Placement

Add a compact usage strip in the CLI header below session identity. Do not put
it inside the terminal canvas or overload the terminal connection status bar;
that would risk repeating xterm/FitAddon height regressions.

### V1 visible summary

Example:

```text
Session  48.2k tokens
Latest   16.5k input / 13 output
Cache    99.6% (16.4k read)
Context  17.8k / 272k
Agents   1 subagent
```

Use **Latest call** until interaction grouping is verified.

At narrow widths:

```text
48.2k tokens | 99.6% cache | 1 subagent
```

### Details popover

Show:

- effective input, output, and total;
- cache read, cache write, other input, and fresh input;
- cache/accounting coverage;
- latest call and session;
- model breakdown;
- model/tool/subagent counts;
- subagent usage breakdown;
- AI credits;
- context occupancy and observation time;
- telemetry age; and
- warnings/policy state.

System-prompt tokens show "Not reported by Copilot."

Cache tooltip:

> Share of effective input tokens served from cache. This is not the
> percentage of requests with a cache hit.

Context tooltip:

> Reported by Copilot as of the last model call.

### Polling

Use a dedicated React Query key.

- selected, focused, active turn: poll every 2 seconds;
- selected, focused, idle: poll every 5 seconds;
- hidden/unselected: suspend polling;
- window focus/reconnect: fetch immediately.

Telemetry failure does not alter terminal connection state. Retain the last
browser snapshot as stale with an age indicator while retrying independently.

### Future metadata tree

Add a dismissible side panel or wide popover without changing the measured
xterm container.

- top level: interactions ordered by start time;
- children: plans, model calls, tools, events, and agents;
- nested agents: recursively expandable;
- in-progress missing parent: placeholder reconciled by span ID;
- metadata mode: names, timing, status, usage;
- content disabled: explicit label, never terminal-derived text; and
- large pages: lazy loading and bounded expansion.

The stream is introduced with this phase because incremental nodes justify its
complexity.

### Future content tree

Only after explicit opt-in and retention controls:

- show user prompt and final response;
- link tool request to execution by call ID;
- show arguments/results with truncation;
- escape arbitrary HTML/ANSI/control characters; and
- lazy-load large content.

## Privacy and Operations

### Metadata-only guarantee

Phase 1 must prove with sanitized fixtures and a real installed-CLI probe that
capture false omits:

- prompt text;
- response text;
- tool arguments; and
- tool results.

Also prove:

- `enduser.pseudo.id` never reaches normalized/public models;
- compact tool names/types remain bounded metadata;
- full descriptions/schemas are rejected in metadata mode; and
- Chat launches do not inherit managed telemetry variables.

### File permissions

Do not assume exporter permissions.

Verify in the real kernel image:

- telemetry directory owner and mode;
- exporter-created file owner and mode;
- process UID/GID;
- umask; and
- accessibility from the agent shell.

Document the observed guarantee. The volume is session-isolated but not
agent-proof.

### Deferred HTTPS OTLP collector

Copilot supports private-CA trust through:

```text
OTEL_EXPORTER_OTLP_CERTIFICATE
```

A local HTTPS collector is technically feasible and could provide:

- central ingestion independent of a live kernel;
- tamper resistance after records are pushed;
- multi-harness fan-in; and
- external observability export.

It is rejected for v1 because it adds certificate generation/distribution,
collector lifecycle, buffering, and another service. The trusted personal
deployment must not require manual host certificate installation.

Reconsider it only if those stronger requirements become important; any
certificates must be generated and managed automatically.

### Documentation

Add:

- `docs/TELEMETRY_PROTOCOL.md` for models, routes, bounds, lifecycle, and
  errors;
- `docs/OPERATIONS.md` updates for volume backup, cleanup, size, retention,
  tamper limitations, and recovery; and
- a `docs/TERMINAL_PROTOCOL.md` cross-reference confirming that telemetry is
  not carried in PTY frames.

## Failure Semantics

Telemetry is auxiliary. It must not stop, restart, or apply backpressure to the
CLI unless the managed telemetry storage itself is a required launch invariant
and cannot be created safely.

Rules:

- no file while process starts: `starting`;
- unsupported schema/version: `degraded`;
- malformed completed line: count warning and continue;
- partial line while launch is alive: retain pending bytes;
- partial line after launch death: discard and count warning;
- duplicate conflict: retain one, mark `degraded`;
- checkpoint corrupt but raw replay safe: rebuild and warn;
- checkpoint newer than reader: `degraded`, do not reset totals;
- telemetry volume missing/collision: explicit launch/recovery error;
- kernel temporarily unavailable: `unavailable`;
- content appears in metadata mode: policy conflict, exclude public content;
- field exceeds bound: retain safe metadata and mark truncated/degraded;
- quota exceeded: rotate only after checkpoint, otherwise stop ingestion and
  report degraded; and
- terminal remains independently usable.

Snapshot freshness uses service receipt time if container/service clock skew is
not within the validated tolerance. Preserve source observation time
separately.

## Implementation Phases

### Phase 0: Validation gates and fixtures

Before wiring the UI:

1. capture sanitized `1.0.81-0` fixtures for:
   - inclusive cache accounting;
   - additive cache accounting or authoritative provider fixture;
   - missing cache fields;
   - tools with capture false;
   - nested subagent;
   - failed tool and failed model call;
   - plan mode; and
   - multiple interactions in one long-lived/resumed CLI;
2. verify enterprise-managed telemetry precedence and policy behavior;
3. verify file permissions and umask;
4. verify flush/final-newline behavior on normal stop, container removal, and
   `SIGKILL`;
5. verify `gen_ai.conversation.id` across resume;
6. verify process-overlap behavior during pane respawn; and
7. fix the normalized fixture schema before implementation.

Blocking outcomes:

- no metadata-only guarantee: do not ship durable source storage;
- unresolved accounting convention for a supported provider: expose raw usage
  but no effective totals/cache percentage for that provider;
- ungrouped multiple interactions: keep v1 at Latest call;
- managed policy overrides file capture: report unsupported policy conflict.

### Phase 1: Durable capture and kernel normalization

1. Add `telemetry_volume_identity` to durable sessions with a backward-
   compatible migration.
2. Add telemetry volume create/adopt/mount/delete/orphan cleanup.
3. Add AgentSpace-owned interactive telemetry config to
   `kernels/copilot_launch`.
4. Reserve/scrub OTel environment namespaces and validate file-exporter
   invariants.
5. Add per-launch file identity/lifecycle tracking.
6. Implement bounded incremental JSONL reading.
7. Implement Copilot span/event normalization.
8. Implement provider-aware effective-input accounting.
9. Implement deduplication, hierarchy, start-order selection, subagents, tools,
   context, costs, and health.
10. Implement atomic versioned checkpointing.
11. Add `kernel_host GET /telemetry`.
12. Add fixtures and restart/eviction determinism tests.

### Phase 2: Public summary and CLI header

1. Add typed `agent_host` telemetry proxy.
2. Add typed `client_service GET /sessions/{id}/telemetry`.
3. Return capability-style unavailable states for unsupported sessions.
4. Add WebUI normalized types, API, React Query polling, and formatting.
5. Add responsive CLI header strip and details popover.
6. Keep terminal and telemetry state/retry independent.
7. Add telemetry protocol and operations documentation.
8. Run visual validation in light/dark and narrow/wide layouts.

Phases 1 and 2 constitute v1.

### Phase 3: Metadata interaction tree

1. Validate/enable interaction grouping.
2. Add bounded interaction index/page/detail routes.
3. Add NDJSON telemetry update stream.
4. Render plans, calls, tools, events, and nested agents.
5. Support out-of-order children and in-progress parents.
6. Add model, subagent, tool, and cache-event filters.
7. Keep content capture false.

### Phase 4: Optional content tree

1. Add explicit durable `captureContent` configuration.
2. Add quotas, retention, deletion, redaction, truncation, and privacy warning.
3. Decide encryption-at-rest policy.
4. Parse stringified messages, system instructions, tool arguments/results.
5. Project only typed bounded content.
6. Render user messages, final responses, arguments, and results safely.
7. Validate storage growth with long sessions before allowing enablement.

## Validation Plan

### Parser fixtures

Cover:

- inclusive and additive accounting;
- ambiguous convention;
- cache read/write aliases;
- missing cache reporting;
- reasoning tokens;
- multiple model calls;
- duplicate spans across files;
- conflicting duplicate spans;
- repeated metric observations;
- tool success/failure;
- nested and recursive subagents;
- background completion order;
- plan mode;
- child before parent;
- unknown spans/events;
- malformed JSON;
- live and dead partial lines;
- truncation/replacement;
- multiple process files;
- checkpoint restart;
- checkpoint corruption/newer version;
- index eviction;
- content-policy violation; and
- stringified content fields for Phase 4.

### Accounting invariants

Assert:

- span identity excludes file identity;
- session totals equal unique completed `chat` spans;
- root aggregates never add to child usage;
- cost and nano-AIU follow the same rule;
- subagent usage is a subset of session usage;
- reasoning is not double-counted;
- inclusive and additive calls normalize correctly;
- ambiguous calls remain uncovered/null;
- cache percentage is token-weighted;
- cache writes count as fresh input;
- missing reporting remains nullable;
- metrics cannot inflate span totals;
- start order drives latest/adjacency;
- tree eviction cannot change totals; and
- checkpoint restart reproduces identical totals.

### Runtime integration

Using a real container and installed Copilot CLI:

1. observe telemetry before the interactive process exits;
2. send multiple user interactions;
3. run a tool and verify metadata-only projection;
4. invoke a subagent and verify hierarchy/usage;
5. exercise plan mode;
6. stop/resume the pane;
7. force partial final records and process death;
8. restart `kernel_host`, `agent_host`, and `client_service`;
9. destroy/recreate the kernel with the same telemetry volume;
10. overlap pane generations;
11. attach two browsers and verify no duplicate accounting;
12. verify Chat launch has no managed telemetry env;
13. verify terminal operation during telemetry failure;
14. verify volume adoption/collision/orphan cleanup; and
15. delete the session and verify telemetry-volume removal.

### Security and privacy

- reject configured `OTEL_*` and `COPILOT_OTEL_*`;
- scrub inherited values;
- assert exporter type/path/content policy;
- reject path traversal and unmanaged files;
- bound hostile lines/files/nodes/content;
- discard end-user pseudonymous ID;
- prove metadata mode omits prompt/response/tool payload;
- detect content-bearing policy conflict;
- verify raw records are never public/logged;
- verify agent tampering degrades safely; and
- verify file owner/mode/umask.

### UI

- formatting and null/coverage states;
- latest-call versus session labels;
- cache category semantics/tooltips;
- stale/unavailable/degraded states;
- 2-second/5-second/focus polling;
- narrow/wide layouts;
- terminal rows remain fully visible;
- popover does not cover/resize xterm incorrectly;
- light and dark screenshots; and
- active CLI screenshots using the repository screenshot skill.

Finish implementation with `just check` and the real runtime
restart/adoption test required by the durable-session skill.

## Implementation Surfaces

### Copilot launch

- `kernels/copilot_launch/src/copilot_launch/__init__.py`
  - managed interactive telemetry config;
  - reserved-prefix validation/scrubbing;
  - resolved invariant checks;
  - Chat exclusion;
- `kernels/copilot_launch/tests/test_copilot_launch.py`
  - environment and launch-policy tests.

### Kernel

- `kernels/kernel_host/src/kernel_host/terminal.py`
  - launch IDs, paths, and lifecycle notifications;
- new `kernel_host` telemetry modules
  - protocol, models, reader, checkpoint, Copilot adapter;
- `kernels/kernel_host/src/kernel_host/app.py`
  - normalized snapshot route;
- kernel tests/fixtures
  - parsing, accounting, durability, security.

### Agent host

- `services/agent_host_rs/src/docker_runtime.rs`
  - telemetry volume lifecycle/mount/adoption/cleanup;
- `services/agent_host_rs/src/models.rs`
  - runtime handle and cleanup resource kind;
- `services/agent_host_rs/src/sessions.rs`
  - telemetry snapshot method;
- new or sibling telemetry route module
  - typed internal proxy.

### Client service

- `services/client_service_rs/src/models.rs`
  - `telemetry_volume_identity` and public normalized models;
- `services/client_service_rs/src/store/sqlite.rs`
  - backward-compatible identity migration;
- `services/client_service_rs/src/agent_host.rs`
  - typed snapshot client;
- `services/client_service_rs/src/api.rs`
  - public route and capability semantics.

### WebUI

- `clients/webui/src/types.ts`
  - normalized telemetry types;
- `clients/webui/src/api.ts`
  - snapshot client;
- `clients/webui/src/queries.ts`
  - focus-aware polling/cache;
- `clients/webui/src/CliView.tsx`
  - summary/details;
- `clients/webui/src/cli-view.css`
  - responsive header layout;
- WebUI tests and screenshot fixtures.

### Documentation

- new `docs/TELEMETRY_PROTOCOL.md`;
- update `docs/OPERATIONS.md`;
- cross-reference from `docs/TERMINAL_PROTOCOL.md`.

## Remaining Validation Questions

These are explicit gates, not undefined architecture:

1. What exact span shape represents multiple user interactions in one
   long-lived and resumed CLI?
2. What exact fields does plan mode emit?
3. What error attributes appear for failed tools/model calls?
4. Which accounting convention does each supported provider/wire API export
   through Copilot OTel?
5. Which compaction/truncation events are emitted by `1.0.81-0`?
6. Can enterprise policy override local exporter/content settings?
7. What are exporter flush and final-line guarantees under each termination?
8. Are source/service clocks sufficiently aligned for source-based age?
9. Does `gen_ai.conversation.id` remain stable across `--resume`?
10. What raw file-count/size and checkpoint limits fit representative long
    metadata-only sessions?

Each unresolved question has a safe fallback:

- interaction unknown -> Latest call only;
- accounting unknown -> raw categories, no derived total/percentage;
- optional event missing -> no inferred explicit cause;
- managed policy conflict -> typed unavailable/degraded;
- clock skew -> receipt-time freshness;
- conversation ID change -> retain AgentSpace-session aggregate with a visible
  conversation-generation boundary.

## References

- Installed CLI: `copilot help monitoring`
- GitHub Copilot CLI command reference:
  <https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference>
- OpenTelemetry GenAI semantic conventions:
  <https://opentelemetry.io/docs/specs/semconv/gen-ai/>
- Copilot SDK OpenTelemetry guide:
  <https://github.com/github/copilot-sdk/blob/main/docs/observability/opentelemetry.md>
- Anthropic prompt caching:
  <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- GitHub enterprise managed settings:
  <https://docs.github.com/en/copilot/reference/enterprise-administrators/enterprise-managed-settings>
- Community parser used as corroboration, not as the AgentSpace abstraction:
  <https://github.com/ccusage/ccusage/tree/main/rust/adapters/copilot>
