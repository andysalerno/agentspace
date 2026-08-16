# Telemetry protocol

AgentSpace exposes Copilot CLI usage telemetry as a normalized metadata-only
snapshot that is separate from terminal transport. The current implementation
supports summary polling for interactive `copilot-cli` sessions only. Tree
routes, message/tool content capture, and enterprise-policy certainty are
deferred.

## Source of truth and PTY separation

The only raw source is Copilot's OpenTelemetry JSONL file exporter. AgentSpace
does **not** derive telemetry from:

- tmux scrollback;
- PTY bytes from `/terminal/ws`;
- Copilot stdout/stderr; or
- terminal lifecycle frames.

Each raw file contains one JSON object per line. `kernel_host` currently uses
only `type: "span"` records for normalization. `type: "metric"` records are
ignored, and other record types are counted as warnings.

Telemetry and terminal health are independent. A telemetry failure can return a
degraded or unavailable snapshot without changing terminal state or interrupting
PTY traffic.

## Managed launch policy

Managed telemetry is enabled only for interactive Copilot CLI launches. Before
exec, `kernels/copilot_launch` strips inherited `OTEL_*` and
`COPILOT_OTEL_*` variables and sets this allowlist:

```text
COPILOT_OTEL_ENABLED=true
COPILOT_OTEL_EXPORTER_TYPE=file
COPILOT_OTEL_FILE_EXPORTER_PATH=/var/lib/agentspace/telemetry/<launch_uuid>.jsonl
OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=false
OTEL_RESOURCE_ATTRIBUTES=agentspace.session.id=<runtime_session_id>
```

Current invariants:

- the exporter path must be an absolute `.jsonl` file directly under
  `/var/lib/agentspace/telemetry`;
- the filename must be a canonical UUID;
- one UUID-named file is created per interactive Copilot process launch; and
- `build_chat_launch()` does not inherit or enable telemetry.

The terminal controller stores launch argv and cwd in:

```text
AGENTSPACE_TERMINAL_LAUNCH_ARGV
AGENTSPACE_TERMINAL_LAUNCH_CWD
```

Telemetry discovery itself uses the live pane's
`COPILOT_OTEL_FILE_EXPORTER_PATH`. `kernel_host` reads that value from the pane
environment and treats it as the active launch only when it still matches the
managed path rules above.

### Enterprise-managed policy status

AgentSpace currently **requests** metadata-only file export, but it does not
prove that higher-precedence GitHub enterprise policy cannot override exporter
type, exporter destination, or content-capture settings. That certainty is
deferred.

What the current implementation does enforce:

- it never intentionally enables content capture;
- it never exposes raw source files over a public API; and
- if content-bearing attributes are observed in a managed metadata run, it marks
  `content_mode=policy_conflict`, records
  `warnings.items[].code=content_policy_conflict`, and continues exposing only
  normalized metadata.

Absence of a policy-conflict warning is **not** proof that enterprise policy
could not have changed behavior upstream.

## Managed files and checkpointing

The raw JSONL files and the normalized checkpoint live in the same telemetry
volume at `/var/lib/agentspace/telemetry`.

Current reader bounds are:

- at most 256 managed source files;
- at most 64 MiB of unread raw-file bytes per snapshot pass;
- at most 512 KiB per JSONL line;
- at most 50,000 distinct normalized spans;
- at most 8 MiB for the compressed checkpoint file;
- at most 64 MiB of uncompressed data when loading the compact checkpoint;
- strings truncated to 256 characters; and
- at most 64 metadata-only tool definitions per span.

The checkpoint file is:

```text
/var/lib/agentspace/telemetry/.agentspace-telemetry-checkpoint-v2.zlib
```

It is written atomically by creating a sibling temporary file in the same
directory, flushing and `fsync`ing it, renaming it into place, and then syncing
the directory where supported.

The v2 checkpoint stores a compact normalized payload: spans, shared source-file
tables, file identities and offsets, warnings, degraded reasons, content mode,
source/received/observed metadata, and related restart state. It does **not**
embed a copied `TelemetrySnapshot`.

Checkpoint loads use bounded streaming zlib decompression and reject truncated,
trailing-invalid, or over-limit payloads as `checkpoint_corrupt`. The legacy
`.agentspace-telemetry-checkpoint-v1.json` file is accepted only as migration
input and is rewritten as v2 on the next successful checkpoint write.

### Restart and tail behavior

On restart, `kernel_host`:

- restores from a valid checkpoint and continues from saved byte offsets;
- reads only unread bytes from each saved cursor, capped by the remaining
  per-pass budget, and continues later passes from the last committed offset if
  backlog remains;
- replays managed raw files from the beginning if the checkpoint is corrupt or
  unreadable;
- replays raw files and leaves checkpoint writing disabled if it finds a newer
  checkpoint version it does not understand;
- surfaces checkpoint write I/O failures as runtime/provider failures rather
  than silently dropping durability; and
- never intentionally resets totals to zero.

Incremental tail handling is strict:

- only complete newline-terminated records are ingested;
- a partial final line is retained only while the active launch is still
  running;
- a partial tail is discarded with `partial_record_discarded` once the launch is
  known dead, or when the partial tail belongs to a non-active file; and
- file replacement/truncation is treated as `source_file_changed` and the old
  cursor is sealed instead of being re-read.

## Normalized snapshot model

`client_service`, `agent_host`, and `kernel_host` all use the same normalized
shape:

```text
TelemetrySnapshot
  schema_version
  state
  reason
  content_mode
  source_version
  observed_at
  received_at
  session
  latest_call
  last_interaction
  context
  counts
  subagents
  cache_signal
  reporting
  warnings
```

### State and reason semantics

Server snapshots currently emit:

- `starting`: managed telemetry exists, but no completed model call has been
  normalized yet;
- `live`: a usable snapshot with no degraded reasons;
- `degraded`: a usable partial snapshot with warning-backed data-quality
  problems; and
- `unavailable`: telemetry is unsupported for the current session or the runtime
  is not currently inspectable, even if previously normalized totals remain
  present.

`stale` exists in the public enum but is currently a **WebUI display state**,
not a `kernel_host` output state. The browser shows `stale` when it keeps an
older successful snapshot while polling retries after a later request failure.

`reason` semantics are current-state specific:

- `starting` currently uses `waiting for first completed model call`;
- `unavailable` returns an explicit availability reason while preserving any
  last-known normalized totals already loaded into the snapshot;
- `degraded` returns one primary degraded reason; and
- when several degraded reasons exist, the current primary `reason` is the
  lexicographically smallest degraded-warning code. Use `warnings.items` for the
  full set.

`warnings.total` is the summed warning count. All current warning codes except
`unknown_record` can contribute to `state=degraded`.

### Content mode semantics

The public enum is:

```text
metadata | content | policy_conflict
```

Current managed behavior is narrower:

- `metadata`: expected steady state;
- `policy_conflict`: content-bearing fields or unsafe tool-definition payloads
  were observed in a run that should have been metadata-only; and
- `content`: reserved for future explicit opt-in and not emitted by the current
  managed launch path.

Metadata-only output may still include safe tool inventory entries with `name`
and `type`. Full tool descriptions, schemas, prompts, responses, arguments, and
results remain deferred.

### Timestamp semantics

- `source_version` comes from Copilot's instrumentation scope version when
  present, then from resource attributes such as `service.version`.
- `observed_at` is the latest normalized span end time seen so far.
- `received_at` is when `kernel_host` last ingested a raw line.
- `context.observed_at` comes from the context event timestamp when present, or
  from the enclosing model span's end time as a fallback.

### Nullable and coverage semantics

Telemetry values are deliberately nullable. Unknown stays `null`; AgentSpace
does not turn unknown into zero.

This applies both per call and in aggregates:

- if a derived field cannot be proven for one completed model call, that call's
  derived field is `null`;
- if any counted model call lacks a field required for an aggregate, the
  aggregate field is also `null` rather than a partial sum; and
- `reporting` tells clients how much of the normalized model-call set supplied
  cache, accounting-convention, effective-input, and context data.

`last_interaction` is present in the public model but currently always `null`.
Top-level interaction grouping has been explicitly deferred until it is verified
against multi-interaction/resume fixtures.

### Count semantics

Current count rules are:

- `interactions`: root `invoke_agent` spans plus traces that have spans but no
  root agent span;
- `model_calls`: completed `chat` spans only;
- `tool_calls`: `execute_tool` spans;
- `subagent_invocations`: nested `invoke_agent` spans only;
- `subagent_model_calls`: completed model calls whose nearest agent ancestor is
  a nested `invoke_agent`; and
- `errors`: spans whose OTel status code is an error.

`latest_call` is the completed model call with the greatest `startTime`
(tie-broken by end time, then trace ID, then span ID). It can point to a
subagent call; it is not forced to be a top-level call. It carries the chosen
model/provider/agent identity, timing, cache-reporting state, resolved token
accounting convention, and the normalized one-call usage breakdown.

`context` is the newest reported context snapshot across completed model calls,
not a terminal-derived estimate.

## Exact accounting rules

Authoritative usage currently comes from **unique completed `chat` spans only**.
Usage attributes on `invoke_agent`, `execute_tool`, or other spans are ignored
for totals and latest-call accounting.

### Deduplication

The unique span key is:

```text
(trace_id, span_id)
```

This key is global across all managed files in the telemetry volume.

- identical duplicate spans collapse into one record;
- conflicting duplicates keep the first normalized span, add a
  `duplicate_conflict` warning, and degrade the snapshot; and
- file order and completion order never determine identity.

### Ordering

Raw JSONL order is completion/export order, not execution order. Parents can
appear after children. Current normalization uses:

- `parentSpanId` for ancestry;
- `startTime` for execution order and latest-call selection; and
- `endTime` for completeness and duration.

### Inclusive, additive, and ambiguous providers

`kernel_host` resolves token accounting in this order:

1. an explicit source attribute:
   `github.copilot.token_accounting_convention` or
   `gen_ai.usage.token_accounting_convention`;
2. the default convention for the session:
   - `inclusive` for direct Copilot CLI sessions; or
   - `unknown` when the launch uses a custom `CONNECTION_URL` provider; then
3. a hard additive override when
   `cache_read_input_tokens + cache_write_input_tokens > raw_input_tokens`.

Current field math:

- `raw_input_tokens`: provider-reported input tokens;
- `effective_input_tokens`:
  - inclusive: `raw_input_tokens`;
  - additive: `raw_input_tokens + cache_read + cache_write`;
- `other_input_tokens`:
  - inclusive: `raw_input_tokens - cache_read - cache_write`;
  - additive: `raw_input_tokens`;
- `fresh_input_tokens`:
  - inclusive: `other_input_tokens + cache_write`;
  - additive: `raw_input_tokens + cache_write`;
- `total_tokens`: `effective_input_tokens + output_tokens`; and
- `cache_reuse_percent`: `cache_read_input_tokens / effective_input_tokens * 100`.

If inclusive arithmetic would make `other_input_tokens` negative, AgentSpace
records `invalid_usage_shape` and leaves the affected derived fields `null`.

If the convention stays `unknown`, AgentSpace still exposes raw counts when
present, but `effective_input_tokens`, `other_input_tokens`,
`fresh_input_tokens`, `total_tokens`, and `cache_reuse_percent` remain `null`.

### Token-weighted cache reuse

Session-level and subagent-level `cache_reuse_percent` are computed from summed
tokens, not from an average of per-call percentages:

```text
sum(cache_read_input_tokens) / sum(effective_input_tokens)
```

This makes aggregate cache reuse token-weighted. The aggregate percentage is
only reported when **every** counted model call reported cache fields.

### Subagent subset semantics

`subagents.*` is a subset projection of session usage:

- it includes only model calls whose nearest agent ancestor is a nested
  `invoke_agent`; and
- it is already included inside `session.*`.

So subagent usage is a slice of whole-session usage, not an additive extra.
`subagents.duration_ms` is the summed duration of the nested `invoke_agent`
spans themselves.

## Cache-signal semantics and limitations

`cache_signal` is an inference, not a proof. It never inspects prompts,
responses, terminal text, or provider internals.

Current comparison lane:

```text
(conversation_id or trace_id, nearest_agent_identity, model_or_requested_model)
```

Only the last two completed model calls in the same lane are compared. If there
is no comparable predecessor, or if the latest two comparable calls lack cache
or fresh-input data, the signal is `unknown`.

Current outcomes:

- `healthy`: comparable calls exist and no cache-break heuristic fired;
- `cache_reset_suspected`:
  - `compaction_or_truncation` with medium confidence when the latest call has a
    compaction/truncation event;
  - `reuse_collapsed` with low confidence when reuse drops from at least 50% to
    under 10% and the latest call is at least 50% fresh input; or
  - `context_discontinuity` with medium confidence when the same drop also
    coincides with reported context tokens falling to half or less of the prior
    comparable call;
- `expected_boundary` with `model_changed` is defined in the schema, but because
  the current comparison lane already keys by model identity, an actual model
  change usually results in `unknown` rather than this state; and
- `unknown`: insufficient comparable metadata.

Important limitations:

- the signal does not prove prompt-cache invalidation;
- it cannot see system-prompt tokens or exact provider cache behavior;
- it depends on completed spans only; and
- it is intentionally conservative when reporting coverage is incomplete.

## API routes and polling

Current routes:

| Boundary | Route |
| --- | --- |
| `kernel_host` | `GET /telemetry` |
| `agent_host` | `GET /sessions/{runtime_session_id}/telemetry` |
| `client_service` | `GET /sessions/{session_id}/telemetry` |

Current public availability behavior:

- unknown durable session: `404`;
- existing session with `telemetry_volume_identity = null`: `200` with
  `state=unavailable` and reason `telemetry is unavailable for this session`;
- existing session with no active `agent_host_session_id` yet: `200` with
  `state=unavailable` and reason
  `telemetry runtime is unavailable until the session is recovered`;
- upstream file/runtime transport failure (including checkpoint write I/O
  failure): `503`; and
- successful but partial normalization: `200` with `state=degraded`.

A successful `200` response with `state=unavailable` may still retain
last-known normalized totals from a prior checkpoint or earlier in-process
ingestion. Clients should key liveness from `state`/`reason`, not from whether
aggregate fields are populated.

Internal `kernel_host`/`agent_host` routes can return a harness-specific
unavailable reason. The public `client_service` route intentionally
short-circuits sessions that have no managed telemetry identity.

### WebUI polling

The WebUI polls the normalized session route with a dedicated React Query key:

- every 2 seconds while the selected session looks active;
- every 5 seconds while the selected session is idle but visible;
- not at all for hidden tabs or no selected session; and
- immediately again on window focus and reconnect.

"Active" currently means any of:

- an active turn is present;
- terminal state is `running`; or
- session status is `active`, `busy`, `running`, or `working`.

If a poll fails after a prior success, the browser keeps the last good snapshot
and displays it as `stale` until refresh succeeds again.

## Trust boundary and deferred scope

The telemetry volume is isolated from workspace snapshots and from terminal
frames, but it is still mounted read-write inside the same kernel container as
the agent shell. An agent with shell access can read, delete, corrupt, or forge
raw telemetry files.

Treat telemetry as operational metadata, not tamper-evident billing evidence.

Current metadata-only raw files can still reveal operational details such as:

- model, requested-model, provider, agent, and tool names/types;
- conversation, response, turn, and tool-call IDs;
- timestamps and durations;
- token and cost counters;
- context occupancy and message-count metadata; and
- Copilot CLI version metadata.

They are designed **not** to capture prompt/response/tool content, but routine
privacy handling should still treat them as sensitive metadata.

Explicitly deferred or guarded in the current release:

- interaction-tree routes and tree-node projection;
- non-null `last_interaction`;
- prompt, response, tool-argument, tool-result, and system-instruction capture;
- public raw-source access; and
- any claim that enterprise policy cannot override the requested metadata-only
  exporter settings.
