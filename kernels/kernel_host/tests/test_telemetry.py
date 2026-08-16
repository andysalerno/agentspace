from __future__ import annotations

import json
import zlib
from dataclasses import asdict, dataclass
from typing import TYPE_CHECKING

import pytest
from kernel_host import telemetry as telemetry_module
from kernel_host.telemetry import (
    CacheReportingState,
    CopilotOtelTelemetryProvider,
    TelemetryContentMode,
    TelemetryProviderRuntimeError,
    TelemetryReaderLimits,
    TelemetryRuntimeInfo,
    TelemetryRuntimeState,
    TelemetrySnapshot,
    TelemetryState,
    TelemetryWarningCode,
    TokenAccountingConvention,
)

if TYPE_CHECKING:
    from collections.abc import Callable, Mapping, Sequence
    from pathlib import Path

SOURCE_VERSION = "1.0.81-0"
TRACE_ID = "trace-1"
ROOT_SPAN_ID = "root-span"
ACTIVE_FILE_ID = "11111111-1111-4111-8111-111111111111"
SECOND_FILE_ID = "22222222-2222-4222-8222-222222222222"
THIRD_FILE_ID = "33333333-3333-4333-8333-333333333333"


@dataclass(slots=True)
class RuntimeInfoStub:
    state: TelemetryRuntimeState = TelemetryRuntimeState.IDLE
    active_launch_id: str | None = None
    active_launch_path: str | None = None
    reason: str | None = None

    async def __call__(self) -> TelemetryRuntimeInfo:
        return TelemetryRuntimeInfo(
            state=self.state,
            active_launch_id=self.active_launch_id,
            active_launch_path=self.active_launch_path,
            reason=self.reason,
        )


def _timestamp(seconds: int) -> list[int]:
    return [seconds, 0]


def _span_record(  # noqa: PLR0913
    *,
    trace_id: str,
    span_id: str,
    name: str,
    start_seconds: int,
    end_seconds: int,
    parent_span_id: str | None = None,
    attributes: Mapping[str, object] | None = None,
    events: Sequence[Mapping[str, object]] | None = None,
    status: str | int = "OK",
) -> dict[str, object]:
    record: dict[str, object] = {
        "type": "span",
        "traceId": trace_id,
        "spanId": span_id,
        "name": name,
        "startTime": _timestamp(start_seconds),
        "endTime": _timestamp(end_seconds),
        "attributes": dict(attributes or {}),
        "events": list(events or []),
        "status": {"code": status},
        "resource": {"attributes": {"service.version": SOURCE_VERSION}},
        "instrumentationScope": {"name": "copilot-cli", "version": SOURCE_VERSION},
    }
    if parent_span_id is not None:
        record["parentSpanId"] = parent_span_id
    return record


def _chat_attributes(  # noqa: PLR0913
    *,
    raw_input_tokens: int,
    output_tokens: int,
    model: str = "gpt-5.6-sol",
    requested_model: str | None = None,
    cache_read_tokens: int | None = None,
    cache_write_tokens: int | None = None,
    nano_aiu: int | None = None,
    opaque_cost: float | None = None,
    conversation_id: str = "conversation-1",
) -> dict[str, object]:
    attributes: dict[str, object] = {
        "gen_ai.response.model": model,
        "gen_ai.request.model": requested_model or model,
        "gen_ai.usage.input_tokens": raw_input_tokens,
        "gen_ai.usage.output_tokens": output_tokens,
        "gen_ai.conversation.id": conversation_id,
    }
    if cache_read_tokens is not None:
        attributes["gen_ai.usage.cache_read.input_tokens"] = cache_read_tokens
    if cache_write_tokens is not None:
        attributes["gen_ai.usage.cache_creation.input_tokens"] = cache_write_tokens
    if nano_aiu is not None:
        attributes["github.copilot.nano_aiu"] = nano_aiu
    if opaque_cost is not None:
        attributes["github.copilot.cost"] = opaque_cost
    return attributes


def _agent_attributes(
    name: str = "root", agent_id: str = "builtin:root"
) -> dict[str, str]:
    return {
        "gen_ai.agent.id": agent_id,
        "gen_ai.agent.name": name,
    }


def _tool_attributes(name: str, tool_type: str = "mcp") -> dict[str, str]:
    return {
        "gen_ai.tool.name": name,
        "gen_ai.tool.type": tool_type,
        "gen_ai.tool.call.id": f"tool-call-{name}",
    }


def _context_event(
    *,
    tokens: int,
    limit: int,
    message_count: int,
    at_seconds: int,
) -> dict[str, object]:
    return {
        "name": "github.copilot.session.usage_info",
        "time": _timestamp(at_seconds),
        "attributes": {
            "github.copilot.current_tokens": tokens,
            "github.copilot.token_limit": limit,
            "github.copilot.messages_length": message_count,
        },
    }


def _write_jsonl(
    path: Path,
    records: list[dict[str, object]],
    *,
    trailing: str = "",
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(_jsonl_text(records, trailing=trailing), encoding="utf-8")


def _jsonl_text(
    records: Sequence[Mapping[str, object]],
    *,
    trailing: str = "",
) -> str:
    return (
        "".join(json.dumps(record, separators=(",", ":")) + "\n" for record in records)
        + trailing
    )


def _warning_count(snapshot: TelemetrySnapshot, code: TelemetryWarningCode) -> int:
    for item in snapshot.warnings.items:
        if item.code == code:
            return item.count
    return 0


def _provider(
    telemetry_dir: Path,
    runtime_info: RuntimeInfoStub,
    *,
    default_convention: TokenAccountingConvention = TokenAccountingConvention.INCLUSIVE,
    limits: TelemetryReaderLimits | None = None,
) -> CopilotOtelTelemetryProvider:
    return CopilotOtelTelemetryProvider(
        telemetry_dir=telemetry_dir,
        runtime_info_provider=runtime_info,
        default_token_accounting_convention=default_convention,
        limits=limits,
    )


@pytest.mark.asyncio
async def test_numeric_otel_status_counts_only_error_code(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    records = [
        _span_record(
            trace_id=TRACE_ID,
            span_id=f"tool-{status}",
            name="execute_tool bash",
            start_seconds=status + 1,
            end_seconds=status + 2,
            attributes=_tool_attributes("bash"),
            status=status,
        )
        for status in (0, 1, 2)
    ]
    _write_jsonl(managed, records)

    snapshot = await _provider(telemetry_dir, RuntimeInfoStub()).snapshot()

    assert snapshot.counts.tool_calls == 3
    assert snapshot.counts.errors == 1


@pytest.mark.asyncio
async def test_provider_normalizes_inclusive_usage_and_ignores_unmanaged_files(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    unmanaged = telemetry_dir / "not-a-uuid.jsonl"
    runtime_info = RuntimeInfoStub()

    root_span = _span_record(
        trace_id=TRACE_ID,
        span_id=ROOT_SPAN_ID,
        name="invoke_agent",
        start_seconds=1,
        end_seconds=10,
        attributes={
            **_agent_attributes(),
            "gen_ai.usage.input_tokens": 999,
            "gen_ai.usage.output_tokens": 999,
            "github.copilot.nano_aiu": 999,
        },
    )
    latest_started = _span_record(
        trace_id=TRACE_ID,
        span_id="chat-latest",
        parent_span_id=ROOT_SPAN_ID,
        name="chat gpt-5.6-sol",
        start_seconds=5,
        end_seconds=6,
        attributes=_chat_attributes(
            raw_input_tokens=50,
            output_tokens=10,
            cache_read_tokens=20,
            cache_write_tokens=5,
            nano_aiu=100,
            opaque_cost=0.5,
        ),
    )
    tool_span = _span_record(
        trace_id=TRACE_ID,
        span_id="tool-1",
        parent_span_id=ROOT_SPAN_ID,
        name="execute_tool bash",
        start_seconds=4,
        end_seconds=5,
        attributes=_tool_attributes("bash"),
    )
    earlier_started = _span_record(
        trace_id=TRACE_ID,
        span_id="chat-earlier",
        parent_span_id=ROOT_SPAN_ID,
        name="chat gpt-5.6-sol",
        start_seconds=1,
        end_seconds=9,
        attributes=_chat_attributes(
            raw_input_tokens=100,
            output_tokens=20,
            cache_read_tokens=60,
            cache_write_tokens=10,
            nano_aiu=200,
            opaque_cost=1.5,
        ),
        events=[_context_event(tokens=400, limit=1000, message_count=5, at_seconds=9)],
    )
    ignored = _span_record(
        trace_id="ignored-trace",
        span_id="ignored-span",
        name="chat gpt-5.6-sol",
        start_seconds=20,
        end_seconds=21,
        attributes=_chat_attributes(raw_input_tokens=9999, output_tokens=9999),
    )
    _write_jsonl(managed, [latest_started, tool_span, root_span, earlier_started])
    _write_jsonl(unmanaged, [ignored])

    snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert snapshot.state == TelemetryState.LIVE
    assert snapshot.source_version == SOURCE_VERSION
    assert snapshot.counts.interactions == 1
    assert snapshot.counts.model_calls == 2
    assert snapshot.counts.tool_calls == 1
    assert snapshot.counts.subagent_invocations == 0
    assert snapshot.reporting.model_calls == 2
    assert snapshot.reporting.cache_reported_calls == 2
    assert snapshot.reporting.convention_resolved_calls == 2
    assert snapshot.reporting.effective_input_covered_calls == 2
    assert snapshot.reporting.context_reported is True
    assert snapshot.session.raw_input_tokens == 150
    assert snapshot.session.effective_input_tokens == 150
    assert snapshot.session.output_tokens == 30
    assert snapshot.session.total_tokens == 180
    assert snapshot.session.cache_read_input_tokens == 80
    assert snapshot.session.cache_write_input_tokens == 15
    assert snapshot.session.other_input_tokens == 55
    assert snapshot.session.fresh_input_tokens == 70
    assert snapshot.session.nano_aiu == 300
    assert snapshot.session.opaque_cost == pytest.approx(2.0)
    assert snapshot.session.cache_reuse_percent == pytest.approx(80 / 150 * 100)
    assert snapshot.context is not None
    assert snapshot.context.tokens == 400
    assert snapshot.latest_call is not None
    assert snapshot.latest_call.started_at == "1970-01-01T00:00:05Z"
    assert snapshot.latest_call.usage.output_tokens == 10
    assert snapshot.latest_call.cache_reporting == CacheReportingState.REPORTED
    assert (
        snapshot.latest_call.token_accounting_convention
        == TokenAccountingConvention.INCLUSIVE
    )


@pytest.mark.asyncio
async def test_provider_continues_ingesting_when_lifetime_file_size_exceeds_budget(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()

    first_record = _span_record(
        trace_id=TRACE_ID,
        span_id="chat-1",
        name="chat gpt-5.6-sol",
        start_seconds=1,
        end_seconds=2,
        attributes=_chat_attributes(raw_input_tokens=10, output_tokens=2),
    )
    later_records = [
        _span_record(
            trace_id=TRACE_ID,
            span_id="chat-2",
            name="chat gpt-5.6-sol",
            start_seconds=3,
            end_seconds=4,
            attributes=_chat_attributes(raw_input_tokens=20, output_tokens=3),
        ),
        _span_record(
            trace_id=TRACE_ID,
            span_id="chat-3",
            name="chat gpt-5.6-sol",
            start_seconds=5,
            end_seconds=6,
            attributes=_chat_attributes(raw_input_tokens=30, output_tokens=4),
        ),
    ]
    first_chunk = _jsonl_text([first_record])
    later_chunk = _jsonl_text(later_records)
    first_bytes = len(first_chunk.encode("utf-8"))
    later_bytes = len(later_chunk.encode("utf-8"))
    budget = max(first_bytes, later_bytes) + 1

    managed.parent.mkdir(parents=True, exist_ok=True)
    managed.write_text(first_chunk, encoding="utf-8")
    provider = _provider(
        telemetry_dir,
        runtime_info,
        limits=TelemetryReaderLimits(max_total_bytes=budget),
    )

    first_snapshot = await provider.snapshot()
    managed.write_text(first_chunk + later_chunk, encoding="utf-8")
    second_snapshot = await provider.snapshot()

    assert first_snapshot.counts.model_calls == 1
    assert second_snapshot.counts.model_calls == 3
    assert second_snapshot.session.raw_input_tokens == 60
    assert (
        _warning_count(second_snapshot, TelemetryWarningCode.SIZE_LIMIT_EXCEEDED) == 0
    )


@pytest.mark.asyncio
async def test_provider_incremental_ingestion_seeks_to_cursor_and_reads_only_tail(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()

    first_record = _span_record(
        trace_id=TRACE_ID,
        span_id="chat-1",
        name="chat gpt-5.6-sol",
        start_seconds=1,
        end_seconds=2,
        attributes=_chat_attributes(raw_input_tokens=10, output_tokens=2),
    )
    second_record = _span_record(
        trace_id=TRACE_ID,
        span_id="chat-2",
        name="chat gpt-5.6-sol",
        start_seconds=3,
        end_seconds=4,
        attributes=_chat_attributes(raw_input_tokens=20, output_tokens=3),
    )
    first_chunk = _jsonl_text([first_record])
    second_chunk = _jsonl_text([second_record])
    first_bytes = len(first_chunk.encode("utf-8"))
    second_bytes = len(second_chunk.encode("utf-8"))

    managed.parent.mkdir(parents=True, exist_ok=True)
    managed.write_text(first_chunk, encoding="utf-8")
    provider = _provider(
        telemetry_dir,
        runtime_info,
        limits=TelemetryReaderLimits(
            max_total_bytes=max(first_bytes, second_bytes) + 1
        ),
    )
    await provider.snapshot()
    managed.write_text(first_chunk + second_chunk, encoding="utf-8")

    calls: list[tuple[Path, int, int]] = []
    original_read_bounded_bytes: Callable[..., bytes] = telemetry_module.__dict__[
        "_read_bounded_bytes"
    ]

    def tracking_read_bounded_bytes(
        path: Path,
        *,
        start_offset: int,
        max_bytes: int,
    ) -> bytes:
        calls.append((path, start_offset, max_bytes))
        return original_read_bounded_bytes(
            path,
            start_offset=start_offset,
            max_bytes=max_bytes,
        )

    monkeypatch.setattr(
        telemetry_module,
        "_read_bounded_bytes",
        tracking_read_bounded_bytes,
    )
    snapshot = await provider.snapshot()

    assert snapshot.counts.model_calls == 2
    assert calls == [(managed, first_bytes, second_bytes)]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    (
        "default_convention",
        "raw_input_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "expected_effective_input",
        "expected_other_input",
        "expected_fresh_input",
        "expected_total_tokens",
        "expected_cache_reuse_percent",
        "expected_resolved_calls",
    ),
    [
        (
            TokenAccountingConvention.ADDITIVE,
            40,
            50,
            10,
            100,
            40,
            50,
            105,
            50.0,
            1,
        ),
        (
            TokenAccountingConvention.UNKNOWN,
            100,
            20,
            10,
            None,
            None,
            None,
            None,
            None,
            0,
        ),
    ],
)
async def test_provider_supports_additive_and_ambiguous_cache_accounting(  # noqa: PLR0913, PLR0917
    tmp_path: Path,
    default_convention: TokenAccountingConvention,
    raw_input_tokens: int,
    cache_read_tokens: int,
    cache_write_tokens: int,
    expected_effective_input: int | None,
    expected_other_input: int | None,
    expected_fresh_input: int | None,
    expected_total_tokens: int | None,
    expected_cache_reuse_percent: float | None,
    expected_resolved_calls: int,
) -> None:
    telemetry_dir = tmp_path / default_convention.value
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()

    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(
                    raw_input_tokens=raw_input_tokens,
                    output_tokens=5,
                    cache_read_tokens=cache_read_tokens,
                    cache_write_tokens=cache_write_tokens,
                ),
            ),
        ],
    )

    snapshot = await _provider(
        telemetry_dir,
        runtime_info,
        default_convention=default_convention,
    ).snapshot()

    assert snapshot.latest_call is not None
    assert snapshot.latest_call.token_accounting_convention == default_convention
    assert snapshot.session.raw_input_tokens == raw_input_tokens
    assert snapshot.session.cache_read_input_tokens == cache_read_tokens
    assert snapshot.session.cache_write_input_tokens == cache_write_tokens
    assert snapshot.session.effective_input_tokens == expected_effective_input
    assert snapshot.session.other_input_tokens == expected_other_input
    assert snapshot.session.fresh_input_tokens == expected_fresh_input
    assert snapshot.session.total_tokens == expected_total_tokens
    assert snapshot.reporting.convention_resolved_calls == expected_resolved_calls
    assert snapshot.reporting.effective_input_covered_calls == (
        1 if expected_effective_input is not None else 0
    )
    if expected_cache_reuse_percent is None:
        assert snapshot.session.cache_reuse_percent is None
    else:
        assert snapshot.session.cache_reuse_percent == pytest.approx(
            expected_cache_reuse_percent,
        )


@pytest.mark.asyncio
async def test_provider_deduplicates_globally_and_keeps_first_conflicting_span(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    runtime_info = RuntimeInfoStub()
    first = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    second = telemetry_dir / f"{SECOND_FILE_ID}.jsonl"
    third = telemetry_dir / f"{THIRD_FILE_ID}.jsonl"

    original = _span_record(
        trace_id=TRACE_ID,
        span_id="chat-duplicate",
        name="chat gpt-5.6-sol",
        start_seconds=1,
        end_seconds=2,
        attributes=_chat_attributes(raw_input_tokens=10, output_tokens=1),
    )
    conflicting = _span_record(
        trace_id=TRACE_ID,
        span_id="chat-duplicate",
        name="chat gpt-5.6-sol",
        start_seconds=1,
        end_seconds=2,
        attributes=_chat_attributes(raw_input_tokens=10, output_tokens=2),
    )
    _write_jsonl(first, [original])
    _write_jsonl(second, [original])
    _write_jsonl(third, [conflicting])

    snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert snapshot.state == TelemetryState.DEGRADED
    assert snapshot.counts.model_calls == 1
    assert snapshot.session.output_tokens == 1
    assert _warning_count(snapshot, TelemetryWarningCode.DUPLICATE_CONFLICT) == 1


@pytest.mark.asyncio
async def test_provider_counts_subagents_and_nested_tools_without_double_counting(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()

    root = _span_record(
        trace_id=TRACE_ID,
        span_id=ROOT_SPAN_ID,
        name="invoke_agent",
        start_seconds=1,
        end_seconds=10,
        attributes=_agent_attributes("root", "builtin:root"),
    )
    root_chat = _span_record(
        trace_id=TRACE_ID,
        span_id="root-chat",
        parent_span_id=ROOT_SPAN_ID,
        name="chat gpt-5.6-sol",
        start_seconds=2,
        end_seconds=3,
        attributes=_chat_attributes(
            raw_input_tokens=20,
            output_tokens=5,
            cache_read_tokens=5,
            cache_write_tokens=0,
        ),
    )
    task_tool = _span_record(
        trace_id=TRACE_ID,
        span_id="task-tool",
        parent_span_id=ROOT_SPAN_ID,
        name="execute_tool task",
        start_seconds=3,
        end_seconds=4,
        attributes=_tool_attributes("task"),
    )
    subagent = _span_record(
        trace_id=TRACE_ID,
        span_id="subagent",
        parent_span_id="task-tool",
        name="invoke_agent task",
        start_seconds=4,
        end_seconds=8,
        attributes=_agent_attributes("task", "builtin:task"),
    )
    subagent_tool = _span_record(
        trace_id=TRACE_ID,
        span_id="subagent-tool",
        parent_span_id="subagent",
        name="execute_tool bash",
        start_seconds=5,
        end_seconds=6,
        attributes=_tool_attributes("bash"),
    )
    subagent_chat = _span_record(
        trace_id=TRACE_ID,
        span_id="subagent-chat",
        parent_span_id="subagent",
        name="chat claude-haiku-4.5",
        start_seconds=6,
        end_seconds=7,
        attributes=_chat_attributes(
            raw_input_tokens=30,
            output_tokens=7,
            model="claude-haiku-4.5",
            cache_read_tokens=10,
            cache_write_tokens=5,
        ),
    )
    _write_jsonl(
        managed,
        [subagent_chat, subagent_tool, subagent, task_tool, root_chat, root],
    )

    snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert snapshot.counts.model_calls == 2
    assert snapshot.counts.tool_calls == 2
    assert snapshot.counts.subagent_invocations == 1
    assert snapshot.counts.subagent_model_calls == 1
    assert snapshot.session.raw_input_tokens == 50
    assert snapshot.subagents.invocations == 1
    assert snapshot.subagents.model_calls == 1
    assert snapshot.subagents.effective_input_tokens == 30
    assert snapshot.subagents.output_tokens == 7
    assert snapshot.subagents.cache_read_input_tokens == 10
    assert snapshot.subagents.cache_write_input_tokens == 5
    assert snapshot.latest_call is not None
    assert snapshot.latest_call.is_subagent is True
    assert snapshot.latest_call.agent_name == "task"


@pytest.mark.asyncio
async def test_provider_retains_live_partial_records_and_discards_dead_tails(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub(
        state=TelemetryRuntimeState.RUNNING,
        active_launch_id=ACTIVE_FILE_ID,
        active_launch_path=str(managed),
    )
    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(raw_input_tokens=10, output_tokens=2),
            ),
        ],
        trailing='{"type":"span"',
    )

    provider = _provider(telemetry_dir, runtime_info)
    live_snapshot = await provider.snapshot()

    assert live_snapshot.state == TelemetryState.LIVE
    assert (
        _warning_count(
            live_snapshot,
            TelemetryWarningCode.PARTIAL_RECORD_DISCARDED,
        )
        == 0
    )

    runtime_info.state = TelemetryRuntimeState.IDLE
    runtime_info.active_launch_id = None
    runtime_info.active_launch_path = None
    dead_snapshot = await provider.snapshot()

    assert dead_snapshot.state == TelemetryState.DEGRADED
    assert dead_snapshot.counts.model_calls == 1
    assert (
        _warning_count(
            dead_snapshot,
            TelemetryWarningCode.PARTIAL_RECORD_DISCARDED,
        )
        == 1
    )


@pytest.mark.asyncio
async def test_provider_surfaces_runtime_unavailable_without_dropping_totals(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()
    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(raw_input_tokens=20, output_tokens=3),
            ),
        ],
    )

    provider = _provider(telemetry_dir, runtime_info)
    live_snapshot = await provider.snapshot()

    runtime_info.state = TelemetryRuntimeState.UNAVAILABLE
    runtime_info.reason = (
        "telemetry runtime is unavailable until the session is recovered"
    )
    unavailable_snapshot = await provider.snapshot()

    assert live_snapshot.state == TelemetryState.LIVE
    assert unavailable_snapshot.state == TelemetryState.UNAVAILABLE
    assert unavailable_snapshot.reason == runtime_info.reason
    assert unavailable_snapshot.counts.model_calls == live_snapshot.counts.model_calls
    assert (
        unavailable_snapshot.session.total_tokens == live_snapshot.session.total_tokens
    )


@pytest.mark.asyncio
async def test_provider_degrades_on_malformed_records_and_content_policy_conflicts(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()

    managed.parent.mkdir(parents=True, exist_ok=True)
    managed.write_text(
        "{not json}\n"
        + json.dumps(
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes={
                    **_chat_attributes(raw_input_tokens=10, output_tokens=2),
                    "gen_ai.input.messages": "sensitive",
                },
            ),
            separators=(",", ":"),
        )
        + "\n",
        encoding="utf-8",
    )

    snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert snapshot.state == TelemetryState.DEGRADED
    assert snapshot.content_mode == TelemetryContentMode.POLICY_CONFLICT
    assert snapshot.counts.model_calls == 1
    assert _warning_count(snapshot, TelemetryWarningCode.MALFORMED_RECORD) == 1
    assert _warning_count(snapshot, TelemetryWarningCode.CONTENT_POLICY_CONFLICT) == 1


@pytest.mark.asyncio
async def test_provider_degrades_on_invalid_utf8_jsonl_records(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()

    managed.parent.mkdir(parents=True, exist_ok=True)
    valid = json.dumps(
        _span_record(
            trace_id=TRACE_ID,
            span_id="chat-valid",
            name="chat gpt-5.6-sol",
            start_seconds=2,
            end_seconds=3,
            attributes=_chat_attributes(raw_input_tokens=10, output_tokens=2),
        ),
        separators=(",", ":"),
    ).encode("utf-8")
    managed.write_bytes(b"\xff\xfe\n" + valid + b"\n")

    snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert snapshot.state == TelemetryState.DEGRADED
    assert snapshot.counts.model_calls == 1
    assert _warning_count(snapshot, TelemetryWarningCode.MALFORMED_RECORD) == 1


@pytest.mark.asyncio
async def test_provider_sanitizes_hostile_numeric_fields_for_json_and_checkpoint(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()

    hostile = _span_record(
        trace_id=TRACE_ID,
        span_id="chat-hostile",
        name="chat gpt-5.6-sol",
        start_seconds=1,
        end_seconds=2,
        attributes={
            **_agent_attributes(),
            "gen_ai.response.model": "gpt-5.6-sol",
            "gen_ai.request.model": "gpt-5.6-sol",
            "gen_ai.usage.input_tokens": -1,
            "gen_ai.usage.output_tokens": 2**65,
            "gen_ai.usage.cache_read.input_tokens": -5,
            "gen_ai.usage.cache_creation.input_tokens": 2**65,
            "github.copilot.nano_aiu": -2,
            "github.copilot.cost": float("nan"),
        },
        events=[
            {
                "name": "github.copilot.session.usage_info",
                "time": _timestamp(2),
                "attributes": {
                    "github.copilot.current_tokens": -1,
                    "github.copilot.token_limit": 2**65,
                    "github.copilot.messages_length": 3,
                },
            }
        ],
    )
    _write_jsonl(managed, [hostile])

    snapshot = await _provider(telemetry_dir, runtime_info).snapshot()
    replayed = await _provider(telemetry_dir, runtime_info).snapshot()

    assert snapshot.state == TelemetryState.DEGRADED
    assert replayed == snapshot
    assert snapshot.counts.model_calls == 1
    assert snapshot.latest_call is not None
    assert snapshot.context is not None
    assert snapshot.session.raw_input_tokens is None
    assert snapshot.session.output_tokens is None
    assert snapshot.session.cache_read_input_tokens is None
    assert snapshot.session.cache_write_input_tokens is None
    assert snapshot.session.nano_aiu is None
    assert snapshot.session.opaque_cost is None
    assert snapshot.session.total_tokens is None
    assert snapshot.latest_call.usage.opaque_cost is None
    assert snapshot.context.tokens is None
    assert snapshot.context.limit is None
    assert snapshot.context.message_count == 3
    assert _warning_count(snapshot, TelemetryWarningCode.INVALID_USAGE_SHAPE) >= 1
    assert json.dumps(asdict(snapshot), allow_nan=False)


@pytest.mark.asyncio
async def test_provider_raises_runtime_error_when_checkpoint_write_fails(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()
    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(raw_input_tokens=20, output_tokens=3),
            ),
        ],
    )

    def fail_checkpoint_write(_path: Path, _payload: bytes) -> None:
        msg = "read-only file system"
        raise OSError(msg)

    monkeypatch.setattr(
        telemetry_module,
        "_atomic_write_bytes",
        fail_checkpoint_write,
    )

    with pytest.raises(TelemetryProviderRuntimeError, match="read-only file system"):
        await _provider(telemetry_dir, runtime_info).snapshot()


@pytest.mark.asyncio
async def test_provider_restarts_from_checkpoint_and_recovers_from_corruption(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()
    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(raw_input_tokens=20, output_tokens=3),
            ),
        ],
    )

    first_provider = _provider(telemetry_dir, runtime_info)
    first_snapshot = await first_provider.snapshot()
    second_snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert first_provider.checkpoint_path.is_file()
    assert second_snapshot.state == TelemetryState.LIVE
    assert second_snapshot.session.total_tokens == first_snapshot.session.total_tokens
    assert second_snapshot.session.raw_input_tokens == 20

    first_provider.checkpoint_path.write_text("{not json", encoding="utf-8")
    corrupted_snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert corrupted_snapshot.state == TelemetryState.DEGRADED
    assert (
        corrupted_snapshot.session.total_tokens == first_snapshot.session.total_tokens
    )
    assert (
        _warning_count(
            corrupted_snapshot,
            TelemetryWarningCode.CHECKPOINT_CORRUPT,
        )
        == 1
    )


@pytest.mark.asyncio
async def test_provider_restarts_from_invalid_utf8_checkpoint(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()
    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(raw_input_tokens=20, output_tokens=3),
            ),
        ],
    )

    first_snapshot = await _provider(telemetry_dir, runtime_info).snapshot()
    checkpoint_path = _provider(telemetry_dir, runtime_info).checkpoint_path
    checkpoint_path.write_bytes(zlib.compress(b"\xff\xfe", level=9))

    corrupted_snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert corrupted_snapshot.state == TelemetryState.DEGRADED
    assert (
        corrupted_snapshot.session.total_tokens == first_snapshot.session.total_tokens
    )
    assert (
        _warning_count(
            corrupted_snapshot,
            TelemetryWarningCode.CHECKPOINT_CORRUPT,
        )
        == 1
    )


@pytest.mark.asyncio
async def test_provider_rejects_compact_checkpoint_with_trailing_garbage(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()
    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(raw_input_tokens=20, output_tokens=3),
            ),
        ],
    )

    provider = _provider(telemetry_dir, runtime_info)
    first_snapshot = await provider.snapshot()
    provider.checkpoint_path.write_bytes(
        provider.checkpoint_path.read_bytes() + b"junk"
    )

    corrupted_snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert corrupted_snapshot.state == TelemetryState.DEGRADED
    assert (
        corrupted_snapshot.session.total_tokens == first_snapshot.session.total_tokens
    )
    assert (
        _warning_count(
            corrupted_snapshot,
            TelemetryWarningCode.CHECKPOINT_CORRUPT,
        )
        == 1
    )


@pytest.mark.asyncio
async def test_provider_rejects_compact_checkpoint_exceeding_uncompressed_limit(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()
    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(raw_input_tokens=20, output_tokens=3),
            ),
        ],
    )

    provider = _provider(telemetry_dir, runtime_info)
    first_snapshot = await provider.snapshot()
    payload = json.loads(
        zlib.decompress(provider.checkpoint_path.read_bytes()).decode("utf-8")
    )
    payload["padding"] = "x" * 4096
    provider.checkpoint_path.write_bytes(
        zlib.compress(
            json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8"),
            level=9,
        ),
    )

    corrupted_snapshot = await _provider(
        telemetry_dir,
        runtime_info,
        limits=TelemetryReaderLimits(max_checkpoint_uncompressed_bytes=1024),
    ).snapshot()

    assert corrupted_snapshot.state == TelemetryState.DEGRADED
    assert (
        corrupted_snapshot.session.total_tokens == first_snapshot.session.total_tokens
    )
    assert (
        _warning_count(
            corrupted_snapshot,
            TelemetryWarningCode.CHECKPOINT_CORRUPT,
        )
        == 1
    )


@pytest.mark.asyncio
async def test_provider_writes_compact_high_span_checkpoints_with_exact_restart_totals(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()

    managed.parent.mkdir(parents=True, exist_ok=True)
    with managed.open("w", encoding="utf-8") as handle:
        for index in range(12_000):
            handle.write(
                json.dumps(
                    _span_record(
                        trace_id=f"trace-{index // 4}",
                        span_id=f"chat-{index}",
                        name="chat gpt-5.6-sol",
                        start_seconds=index + 1,
                        end_seconds=index + 2,
                        attributes=_chat_attributes(
                            raw_input_tokens=index + 1,
                            output_tokens=1,
                            cache_read_tokens=1,
                            cache_write_tokens=0,
                            nano_aiu=1,
                            opaque_cost=0.01,
                        ),
                    ),
                    separators=(",", ":"),
                )
                + "\n"
            )

    provider = _provider(telemetry_dir, runtime_info)
    first_snapshot = await provider.snapshot()
    replayed_snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert (
        provider.checkpoint_path.stat().st_size
        <= TelemetryReaderLimits().max_checkpoint_bytes
    )
    assert first_snapshot.counts.model_calls == 12_000
    assert replayed_snapshot == first_snapshot


@pytest.mark.asyncio
async def test_provider_degrades_on_newer_checkpoint_versions_without_resetting_totals(
    tmp_path: Path,
) -> None:
    telemetry_dir = tmp_path / "telemetry"
    managed = telemetry_dir / f"{ACTIVE_FILE_ID}.jsonl"
    runtime_info = RuntimeInfoStub()
    _write_jsonl(
        managed,
        [
            _span_record(
                trace_id=TRACE_ID,
                span_id="chat-1",
                name="chat gpt-5.6-sol",
                start_seconds=1,
                end_seconds=2,
                attributes=_chat_attributes(raw_input_tokens=12, output_tokens=3),
            ),
        ],
    )

    provider = _provider(telemetry_dir, runtime_info)
    first_snapshot = await provider.snapshot()
    provider.checkpoint_path.write_bytes(
        zlib.compress(
            json.dumps({"checkpoint_version": 99}).encode("utf-8"),
            level=9,
        ),
    )

    replayed_snapshot = await _provider(telemetry_dir, runtime_info).snapshot()

    assert replayed_snapshot.state == TelemetryState.DEGRADED
    assert replayed_snapshot.session.total_tokens == first_snapshot.session.total_tokens
    assert (
        _warning_count(
            replayed_snapshot,
            TelemetryWarningCode.CHECKPOINT_NEWER_VERSION,
        )
        == 1
    )
    assert (
        json.loads(
            zlib.decompress(provider.checkpoint_path.read_bytes()).decode("utf-8")
        )["checkpoint_version"]
        == 99
    )
