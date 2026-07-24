# ACP Canonical Stream Project

## Goal

Make Agent Client Protocol (ACP) the canonical streaming format for AgentSpace. The service should no longer translate ACP into the older custom `KernelEvent` stream (`text_delta`, `tool_call`, etc.). Instead, kernels emit ACP-shaped events and downstream services/clients consume those events directly.

ACP is the only supported kernel path. OpenCode is the default ACP server;
GitHub Copilot CLI and custom commands are selected as ACP server profiles.

## Current Findings

- `kernel.events.KernelEvent` is now an ACP-friendly envelope. It still keeps legacy custom fields and constructors for compatibility, but the active lifecycle/control names are `session/start`, `session/status`, `session/error`, and `session/end`.
- `kernel_acp.AcpKernel` speaks ACP to named server profiles and passes ACP `session/update` payloads through as `session/update` events. It emits `session/prompt/result` when a prompt completes.
- `agent_host` and `client_service` type and serialize streams as `KernelEvent` objects.
- `client_service` now builds assistant messages from ACP `session/update` events, including text, reasoning, plan JSON, and tool calls keyed by `toolCallId`. It retains legacy flattening fallback for old event histories/stubs.
- The web UI and CLI UI apply ACP `session/update` events while streaming and keep legacy fallback handling.
- `kernel_host.registry.KERNEL_REGISTRY` registers only ACP. `HarnessName` still contains older enum values temporarily, but `available_harnesses()` and client-facing APIs advertise only `acp`.
- The active `uv`/pytest/ruff/pyright workspace excludes retired non-ACP kernel packages. The legacy `kernel_copilot` parser package has been removed.

## Target Shape

Use an ACP-shaped stream event object throughout the service boundary:

- Preserve ACP `session/update` payloads as close to the protocol as possible:
  - `{"type":"session/update","session_id":"...","update":{...}}`
- Preserve ACP turn completion:
  - `{"type":"session/prompt/result","session_id":"...","result":{"stopReason":"end_turn"}}`
- Preserve service/kernel lifecycle events as AgentSpace control events because ACP itself does not define kernel process lifecycle:
  - `{"type":"session/start","session_id":"...","kernel":"acp"}`
  - `{"type":"session/status","status":"busy|idle|error|done"}`
  - `{"type":"session/end"}`
  - `{"type":"session/error","message":"..."}`

This keeps ACP events intact while retaining the minimum non-ACP envelope needed by AgentSpace services.

## Implementation Plan

1. Replace the shared kernel event model with an ACP-friendly `KernelEvent` dataclass.
   - Keep the name `KernelEvent` to reduce blast radius.
   - Store `type`, `ts`, `session_id`, `kernel`, `status`, `method`, `params`, `update`, `result`, `error`, and `message`.
   - Add constructors for ACP events and lifecycle/control events.
   - Temporarily keep compatibility constructor names where cheap, but migrate active code away from custom event types.

2. Update `kernel_acp` to stop mapping ACP session updates.
   - On `session/update`, emit the update payload directly.
   - On `session/prompt` response, emit `session/prompt/result` with the ACP result object.
   - Keep JSON-RPC request/response handling for initialize, session setup, permissions, fs, and terminal methods.
   - Remove `_tool_names` and text/tool flattening helpers if unused after migration.

3. Make ACP the only active kernel.
   - Update `kernel_host.registry` to import/register only `AcpKernel`.
   - Reduce `HarnessName` to `ACP = "acp"` if tests and services can be updated cleanly.
   - Change user-facing defaults to `acp`.
   - Update package dependencies for `kernel_host` so it no longer depends on non-ACP kernels.

4. Update `agent_host` to serialize and stream the new `KernelEvent` shape.
   - Preserve history as lists of event dictionaries.
   - Ensure status tracking still works for `session/status` events.

5. Update `client_service` to understand ACP events.
   - Replace visible-event filtering with ACP event visibility (`session/update`, `session/error`).
   - Build assistant `MessageRecord` summaries from ACP `session/update` events:
     - `agent_message_chunk` text content -> assistant content.
     - `agent_thought_chunk` text content -> reasoning.
     - `tool_call` / `tool_call_update` -> tool call records keyed by `toolCallId`.
     - `plan` -> preserve in reasoning or message metadata until a first-class plan model exists.
   - Keep public message API stable if feasible so existing clients still get `content`, `reasoning`, and `tool_calls`.

6. Update web UI and CLI UI streaming adapters.
   - Apply ACP `session/update` events directly while streaming.
   - Track tool calls by `toolCallId`, not by title/order.
   - Display text, reasoning, tool statuses, and tool output content.

7. Update tests.
   - Rewrite ACP mapping tests to assert passthrough ACP events.
   - Update service/client tests from old event names to ACP events.
   - Update kernel registry/default harness expectations to ACP-only.

8. Verify.
   - Run `uv run ruff format .`.
   - Run `uv run ruff check .`.
   - Run `uv run pyright`.
   - Run `uv run --all-packages pytest`.
   - Run `just webui-lint` and `just check` when the migration is coherent.

## Progress Log

- 2026-04-27: Created project plan. Initial read shows the main migration points are `kernel.events`, `kernel_acp`, `kernel_host.registry`, `agent_host`, `client_service`, `clients/webui/src/ChatView.tsx`, `clients/webui/src/types.ts`, and `clients/cli_ui/src/cli_ui/app.py`.
- 2026-04-28: Reworked `kernel.events` around ACP-shaped session events and added `session_update()` / `session_prompt_result()` constructors.
- 2026-04-28: Updated `kernel_acp` to pass through ACP updates and prompt results instead of flattening them into text/tool events.
- 2026-04-28: Made ACP the only active kernel registry entry, changed service defaults to ACP, reduced `kernel_host` dependencies to `kernel-acp`, and removed non-ACP kernels from the active workspace/tooling paths.
- 2026-04-28: Updated `client_service`, web UI, and CLI UI stream consumers to understand ACP `session/update` events. Client-facing message summaries still expose `content`, `reasoning`, and `tool_calls`.
- 2026-04-28: Updated tests for ACP passthrough and canonical lifecycle names. Verification passes: `just check` completed successfully, including Python format/lint/type-check/tests, web lint/Knip, and the web build. The web lint step still reports existing React advisory warnings but no errors.

## Open Questions / Risks

- ACP does not define AgentSpace process lifecycle events, so a small AgentSpace control envelope remains necessary unless HTTP response boundaries are used instead.
- Existing persisted message records do not have a first-class ACP raw event field. For this pass, preserve public message summaries and keep full ACP event history in session event history.
- Built frontend artifacts under `clients/webui/dist/` appear present in the repo. Do not edit them manually; let the build regenerate if required.
- `HarnessName` still includes retired non-ACP values because several service models and mount-path tests reference it. The active registry and public harness list are ACP-only; a later cleanup can reduce the enum and delete retired package directories outright.
