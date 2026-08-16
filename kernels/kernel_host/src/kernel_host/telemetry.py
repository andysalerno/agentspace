from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import uuid
from collections import Counter
from contextlib import suppress
from dataclasses import asdict, dataclass, replace
from datetime import UTC, datetime
from enum import StrEnum
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING, Protocol, cast

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable, Iterable, Mapping, Sequence

logger = logging.getLogger(__name__)

TELEMETRY_SNAPSHOT_SCHEMA_VERSION = 1
TELEMETRY_CHECKPOINT_SCHEMA_VERSION = 1
TELEMETRY_CHECKPOINT_FILE_NAME = ".agentspace-telemetry-checkpoint-v1.json"
_MANAGED_TELEMETRY_DIR = PurePosixPath("/var/lib/agentspace/telemetry")
_CACHE_WRITE_ALIASES = (
    "gen_ai.usage.cache_creation.input_tokens",
    "gen_ai.usage.cache_write.input_tokens",
)
_EXPLICIT_ACCOUNTING_KEYS = (
    "github.copilot.token_accounting_convention",
    "gen_ai.usage.token_accounting_convention",
)
_CONTENT_ATTRIBUTE_KEYS = frozenset(
    {
        "gen_ai.input.messages",
        "gen_ai.output.messages",
        "gen_ai.system_instructions",
        "gen_ai.tool.description",
        "gen_ai.tool.call.arguments",
        "gen_ai.tool.call.result",
    },
)
_CONTENT_TOOL_DEFINITION_KEYS = frozenset(
    {
        "description",
        "input_schema",
        "output_schema",
        "parameters",
        "schema",
        "properties",
        "required",
    },
)
_ALLOWED_TOOL_DEFINITION_KEYS = frozenset({"name", "type"})
_TRUNCATION_EVENT_TOKENS = ("compaction", "truncate", "truncation")
_DEGRADED_WARNING_CODES = frozenset(
    {
        "checkpoint_corrupt",
        "checkpoint_newer_version",
        "content_policy_conflict",
        "duplicate_conflict",
        "field_truncated",
        "file_limit_exceeded",
        "invalid_usage_shape",
        "line_too_long",
        "malformed_record",
        "partial_record_discarded",
        "size_limit_exceeded",
        "source_file_changed",
        "span_limit_exceeded",
    },
)


class TelemetryState(StrEnum):
    STARTING = "starting"
    LIVE = "live"
    STALE = "stale"
    UNAVAILABLE = "unavailable"
    DEGRADED = "degraded"


class TelemetryContentMode(StrEnum):
    METADATA = "metadata"
    CONTENT = "content"
    POLICY_CONFLICT = "policy_conflict"


class CacheReportingState(StrEnum):
    REPORTED = "reported"
    UNREPORTED = "unreported"


class TokenAccountingConvention(StrEnum):
    INCLUSIVE = "inclusive"
    ADDITIVE = "additive"
    UNKNOWN = "unknown"


class CacheSignalState(StrEnum):
    HEALTHY = "healthy"
    CACHE_RESET_SUSPECTED = "cache_reset_suspected"
    EXPECTED_BOUNDARY = "expected_boundary"
    UNKNOWN = "unknown"


class CacheSignalConfidence(StrEnum):
    LOW = "low"
    MEDIUM = "medium"


class CacheSignalReason(StrEnum):
    REUSE_COLLAPSED = "reuse_collapsed"
    CONTEXT_DISCONTINUITY = "context_discontinuity"
    COMPACTION_OR_TRUNCATION = "compaction_or_truncation"
    MODEL_CHANGED = "model_changed"


class TelemetryWarningCode(StrEnum):
    CHECKPOINT_CORRUPT = "checkpoint_corrupt"
    CHECKPOINT_NEWER_VERSION = "checkpoint_newer_version"
    CONTENT_POLICY_CONFLICT = "content_policy_conflict"
    DUPLICATE_CONFLICT = "duplicate_conflict"
    FIELD_TRUNCATED = "field_truncated"
    FILE_LIMIT_EXCEEDED = "file_limit_exceeded"
    INVALID_USAGE_SHAPE = "invalid_usage_shape"
    LINE_TOO_LONG = "line_too_long"
    MALFORMED_RECORD = "malformed_record"
    PARTIAL_RECORD_DISCARDED = "partial_record_discarded"
    SIZE_LIMIT_EXCEEDED = "size_limit_exceeded"
    SOURCE_FILE_CHANGED = "source_file_changed"
    SPAN_LIMIT_EXCEEDED = "span_limit_exceeded"
    UNKNOWN_RECORD = "unknown_record"


class _SpanKind(StrEnum):
    AGENT = "agent"
    MODEL_CALL = "model_call"
    TOOL_CALL = "tool_call"
    OTHER = "other"


class TelemetryRuntimeState(StrEnum):
    RUNNING = "running"
    IDLE = "idle"
    UNAVAILABLE = "unavailable"


@dataclass(frozen=True, slots=True)
class UsageBreakdown:
    raw_input_tokens: int | None = None
    effective_input_tokens: int | None = None
    output_tokens: int | None = None
    total_tokens: int | None = None
    reasoning_output_tokens: int | None = None
    cache_read_input_tokens: int | None = None
    cache_write_input_tokens: int | None = None
    other_input_tokens: int | None = None
    fresh_input_tokens: int | None = None
    cache_reuse_percent: float | None = None
    nano_aiu: int | None = None
    opaque_cost: float | None = None


@dataclass(frozen=True, slots=True)
class ModelCallSummary:
    started_at: str | None = None
    ended_at: str | None = None
    duration_ms: int | None = None
    model: str | None = None
    requested_model: str | None = None
    provider: str | None = None
    agent_id: str | None = None
    agent_name: str | None = None
    is_subagent: bool = False
    cache_reporting: CacheReportingState = CacheReportingState.UNREPORTED
    token_accounting_convention: TokenAccountingConvention = (
        TokenAccountingConvention.UNKNOWN
    )
    usage: UsageBreakdown = UsageBreakdown()


@dataclass(frozen=True, slots=True)
class ActivityCounts:
    interactions: int = 0
    model_calls: int = 0
    tool_calls: int = 0
    subagent_invocations: int = 0
    subagent_model_calls: int = 0
    errors: int = 0


@dataclass(frozen=True, slots=True)
class ReportingCoverage:
    model_calls: int = 0
    cache_reported_calls: int = 0
    convention_resolved_calls: int = 0
    effective_input_covered_calls: int = 0
    context_reported: bool = False


@dataclass(frozen=True, slots=True)
class ContextUsage:
    tokens: int | None = None
    limit: int | None = None
    message_count: int | None = None
    observed_at: str | None = None


@dataclass(frozen=True, slots=True)
class SubagentBreakdown:
    invocations: int = 0
    model_calls: int = 0
    effective_input_tokens: int | None = None
    output_tokens: int | None = None
    cache_read_input_tokens: int | None = None
    cache_write_input_tokens: int | None = None
    duration_ms: int | None = None


@dataclass(frozen=True, slots=True)
class CacheSignal:
    state: CacheSignalState = CacheSignalState.UNKNOWN
    confidence: CacheSignalConfidence | None = None
    reason: CacheSignalReason | None = None


@dataclass(frozen=True, slots=True)
class TelemetryWarning:
    code: TelemetryWarningCode
    count: int


@dataclass(frozen=True, slots=True)
class TelemetryWarningSummary:
    total: int = 0
    items: tuple[TelemetryWarning, ...] = ()


@dataclass(frozen=True, slots=True)
class TelemetrySnapshot:
    schema_version: int = TELEMETRY_SNAPSHOT_SCHEMA_VERSION
    state: TelemetryState = TelemetryState.UNAVAILABLE
    reason: str | None = None
    content_mode: TelemetryContentMode = TelemetryContentMode.METADATA
    source_version: str | None = None
    observed_at: str | None = None
    received_at: str | None = None
    session: UsageBreakdown = UsageBreakdown()
    latest_call: ModelCallSummary | None = None
    last_interaction: UsageBreakdown | None = None
    context: ContextUsage | None = None
    counts: ActivityCounts = ActivityCounts()
    subagents: SubagentBreakdown = SubagentBreakdown()
    cache_signal: CacheSignal | None = None
    reporting: ReportingCoverage = ReportingCoverage()
    warnings: TelemetryWarningSummary = TelemetryWarningSummary()


@dataclass(frozen=True, slots=True)
class TelemetryRuntimeInfo:
    state: TelemetryRuntimeState
    active_launch_id: str | None = None
    active_launch_path: str | None = None
    reason: str | None = None


@dataclass(frozen=True, slots=True)
class TelemetryReaderLimits:
    max_files: int = 256
    max_total_bytes: int = 64 * 1024 * 1024
    max_line_bytes: int = 512 * 1024
    max_distinct_spans: int = 50_000
    max_checkpoint_bytes: int = 8 * 1024 * 1024
    max_string_length: int = 256
    max_tool_definitions: int = 64


@dataclass(frozen=True, slots=True)
class _FileCursor:
    path: str
    device: int
    inode: int
    offset: int
    sealed: bool = False

    @property
    def file_name(self) -> str:
        return Path(self.path).name


@dataclass(frozen=True, slots=True)
class _SpanKey:
    trace_id: str
    span_id: str

    def encoded(self) -> str:
        return f"{self.trace_id}:{self.span_id}"


@dataclass(frozen=True, slots=True)
class _SpanProvenance:
    file_name: str
    offset: int


@dataclass(frozen=True, slots=True)
class _DuplicateConflict:
    key: _SpanKey
    first: _SpanProvenance
    second: _SpanProvenance


@dataclass(frozen=True, slots=True)
class _ModelUsageDetails:
    usage: UsageBreakdown
    cache_reporting: CacheReportingState
    token_accounting_convention: TokenAccountingConvention


@dataclass(frozen=True, slots=True)
class _TelemetrySpan:
    key: _SpanKey
    parent_span_id: str | None
    kind: _SpanKind
    name: str | None
    started_at_ns: int
    ended_at_ns: int
    started_at: str
    ended_at: str
    is_error: bool
    model: str | None
    requested_model: str | None
    provider: str | None
    conversation_id: str | None
    interaction_id: str | None
    turn_id: str | None
    response_id: str | None
    previous_response_id: str | None
    agent_id: str | None
    agent_name: str | None
    tool_name: str | None
    tool_type: str | None
    tool_call_id: str | None
    usage: _ModelUsageDetails | None
    context: ContextUsage | None
    has_compaction_or_truncation: bool
    source_version: str | None
    provenance: _SpanProvenance
    digest: str


class SessionTelemetryProvider(Protocol):
    async def snapshot(self) -> TelemetrySnapshot: ...


class TelemetryProviderRuntimeError(RuntimeError):
    """Telemetry source inspection failed unexpectedly."""


class UnavailableSessionTelemetryProvider:
    def __init__(self, reason: str) -> None:
        self._reason = reason

    async def snapshot(self) -> TelemetrySnapshot:
        return unavailable_snapshot(self._reason)


class CopilotOtelTelemetryProvider:
    def __init__(
        self,
        *,
        telemetry_dir: str | Path,
        runtime_info_provider: Callable[[], Awaitable[TelemetryRuntimeInfo]],
        default_token_accounting_convention: TokenAccountingConvention | None = None,
        limits: TelemetryReaderLimits | None = None,
        now: Callable[[], datetime] | None = None,
    ) -> None:
        self._telemetry_dir = Path(telemetry_dir)
        self._runtime_info_provider = runtime_info_provider
        self._default_token_accounting_convention = default_token_accounting_convention
        self._limits = limits or TelemetryReaderLimits()
        self._now = now or _utc_now
        self._lock = asyncio.Lock()
        self._checkpoint_path = self._telemetry_dir / TELEMETRY_CHECKPOINT_FILE_NAME
        self._loaded = False
        self._checkpoint_write_disabled = False
        self._dirty = False
        self._has_source_files = False
        self._spans: dict[str, _TelemetrySpan] = {}
        self._file_cursors: dict[str, _FileCursor] = {}
        self._warning_counts: Counter[str] = Counter()
        self._duplicate_conflicts: list[_DuplicateConflict] = []
        self._degraded_reasons: set[str] = set()
        self._content_mode = TelemetryContentMode.METADATA
        self._source_version: str | None = None
        self._observed_at_ns: int | None = None
        self._received_at: str | None = None

    @property
    def checkpoint_path(self) -> Path:
        return self._checkpoint_path

    async def snapshot(self) -> TelemetrySnapshot:
        async with self._lock:
            runtime_info = await self._runtime_info_provider()
            return await asyncio.to_thread(self._snapshot_sync, runtime_info)

    def _snapshot_sync(self, runtime_info: TelemetryRuntimeInfo) -> TelemetrySnapshot:
        if not self._loaded:
            self._load_or_replay(runtime_info)
            self._loaded = True
        self._ingest_incremental(runtime_info)
        snapshot = self._build_snapshot(runtime_info)
        if self._dirty and not self._checkpoint_write_disabled:
            self._write_checkpoint(snapshot)
            self._dirty = False
        return snapshot

    def _load_or_replay(self, runtime_info: TelemetryRuntimeInfo) -> None:  # noqa: PLR0911
        if not self._checkpoint_path.is_file():
            self._replay_all_files(runtime_info)
            return

        try:
            size = self._checkpoint_path.stat().st_size
        except OSError as error:
            logger.warning("failed to stat telemetry checkpoint: %s", error)
            self._record_warning(TelemetryWarningCode.CHECKPOINT_CORRUPT)
            self._replay_all_files(runtime_info)
            return

        if size > self._limits.max_checkpoint_bytes:
            self._record_warning(TelemetryWarningCode.CHECKPOINT_CORRUPT)
            self._replay_all_files(runtime_info)
            return

        try:
            payload = json.loads(self._checkpoint_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            logger.warning("failed to read telemetry checkpoint: %s", error)
            self._record_warning(TelemetryWarningCode.CHECKPOINT_CORRUPT)
            self._replay_all_files(runtime_info)
            return

        data = _mapping(payload)
        if data is None:
            self._record_warning(TelemetryWarningCode.CHECKPOINT_CORRUPT)
            self._replay_all_files(runtime_info)
            return

        version = _parse_int(data.get("checkpoint_version"))
        if version is None:
            self._record_warning(TelemetryWarningCode.CHECKPOINT_CORRUPT)
            self._replay_all_files(runtime_info)
            return

        if version > TELEMETRY_CHECKPOINT_SCHEMA_VERSION:
            self._record_warning(TelemetryWarningCode.CHECKPOINT_NEWER_VERSION)
            self._checkpoint_write_disabled = True
            self._replay_all_files(runtime_info)
            return

        if version != TELEMETRY_CHECKPOINT_SCHEMA_VERSION:
            self._record_warning(TelemetryWarningCode.CHECKPOINT_CORRUPT)
            self._replay_all_files(runtime_info)
            return

        if not self._restore_checkpoint(data):
            self._record_warning(TelemetryWarningCode.CHECKPOINT_CORRUPT)
            self._replay_all_files(runtime_info)

    def _restore_checkpoint(  # noqa: C901, PLR0911, PLR0912
        self,
        payload: Mapping[str, object],
    ) -> bool:
        warnings = _mapping(payload.get("warnings"))
        if warnings is None:
            return False

        self._warning_counts.clear()
        for code, raw_count in warnings.items():
            if code not in TelemetryWarningCode._value2member_map_:
                continue
            count = _parse_int(raw_count)
            if count is None or count < 0:
                return False
            self._warning_counts[code] = count

        degraded = payload.get("degraded_reasons")
        degraded_list = _string_sequence(degraded)
        if degraded_list is None:
            return False
        self._degraded_reasons = set(degraded_list)

        content_mode_raw = _parse_string(payload.get("content_mode"))
        if content_mode_raw is None:
            return False
        try:
            self._content_mode = TelemetryContentMode(content_mode_raw)
        except ValueError:
            return False

        self._source_version = _parse_string(payload.get("source_version"))
        self._received_at = _parse_string(payload.get("received_at"))
        self._observed_at_ns = _parse_int(payload.get("observed_at_ns"))
        self._has_source_files = bool(payload.get("has_source_files"))

        files = _sequence(payload.get("files"))
        if files is None:
            return False
        self._file_cursors = {}
        for item in files:
            file_mapping = _mapping(item)
            if file_mapping is None:
                return False
            cursor = _deserialize_file_cursor(file_mapping)
            if cursor is None:
                return False
            self._file_cursors[cursor.path] = cursor

        conflicts = _sequence(payload.get("duplicate_conflicts"))
        if conflicts is None:
            return False
        self._duplicate_conflicts = []
        for item in conflicts:
            conflict_mapping = _mapping(item)
            if conflict_mapping is None:
                return False
            conflict = _deserialize_duplicate_conflict(conflict_mapping)
            if conflict is None:
                return False
            self._duplicate_conflicts.append(conflict)

        spans = _sequence(payload.get("spans"))
        if spans is None:
            return False
        self._spans = {}
        for item in spans:
            span_mapping = _mapping(item)
            if span_mapping is None:
                return False
            span = _deserialize_span(span_mapping)
            if span is None:
                return False
            self._spans[span.key.encoded()] = span
            if span.source_version is not None:
                self._source_version = span.source_version
            self._observe_source_time(span.ended_at_ns)
        return True

    def _replay_all_files(self, runtime_info: TelemetryRuntimeInfo) -> None:
        warnings = Counter(self._warning_counts)
        degraded_reasons = set(self._degraded_reasons)
        content_mode = self._content_mode
        source_version = self._source_version
        received_at = self._received_at

        self._spans = {}
        self._file_cursors = {}
        self._duplicate_conflicts = []
        self._warning_counts = warnings
        self._degraded_reasons = degraded_reasons
        self._content_mode = content_mode
        self._source_version = source_version
        self._received_at = received_at
        self._observed_at_ns = None
        self._has_source_files = False
        selected_files = self._discover_selected_files(runtime_info)
        for path in selected_files:
            self._ingest_file(path, cursor=None, runtime_info=runtime_info)

    def _ingest_incremental(self, runtime_info: TelemetryRuntimeInfo) -> None:
        selected_files = self._discover_selected_files(runtime_info)
        current_paths = {str(path) for path in selected_files}
        for path in selected_files:
            cursor = self._file_cursors.get(str(path))
            self._ingest_file(path, cursor=cursor, runtime_info=runtime_info)

        missing_paths = [
            path
            for path in tuple(self._file_cursors)
            if path not in current_paths and not self._file_cursors[path].sealed
        ]
        if missing_paths:
            self._dirty = True

    def _discover_selected_files(  # noqa: C901
        self, runtime_info: TelemetryRuntimeInfo
    ) -> list[Path]:
        if not self._telemetry_dir.exists():
            self._has_source_files = False
            return []
        if not self._telemetry_dir.is_dir():
            msg = f"telemetry path is not a directory: {self._telemetry_dir}"
            raise TelemetryProviderRuntimeError(msg)

        try:
            discovered = [
                entry
                for entry in self._telemetry_dir.iterdir()
                if self._is_managed_jsonl_path(entry)
            ]
        except OSError as error:
            msg = f"failed to inspect telemetry directory: {error}"
            raise TelemetryProviderRuntimeError(msg) from error

        self._has_source_files = bool(discovered)
        active_path = runtime_info.active_launch_path
        priority: dict[str, tuple[int, str]] = {}

        def priority_rank(path: Path) -> tuple[int, str]:
            raw = str(path)
            if active_path == raw:
                return (0, raw)
            if raw in self._file_cursors:
                return (1, raw)
            return (2, raw)

        for path in discovered:
            priority[str(path)] = priority_rank(path)

        ordered = sorted(discovered, key=lambda path: priority[str(path)])
        if len(ordered) > self._limits.max_files:
            self._record_warning(TelemetryWarningCode.FILE_LIMIT_EXCEEDED)
            ordered = ordered[: self._limits.max_files]

        total_bytes = 0
        selected: list[Path] = []
        for path in ordered:
            try:
                size = path.stat().st_size
            except OSError as error:
                msg = f"failed to stat telemetry file {path.name}: {error}"
                raise TelemetryProviderRuntimeError(msg) from error
            if total_bytes + size > self._limits.max_total_bytes:
                self._record_warning(TelemetryWarningCode.SIZE_LIMIT_EXCEEDED)
                break
            total_bytes += size
            selected.append(path)
        return selected

    def _ingest_file(
        self,
        path: Path,
        *,
        cursor: _FileCursor | None,
        runtime_info: TelemetryRuntimeInfo,
    ) -> None:
        try:
            stat_result = path.stat()
        except OSError as error:
            logger.warning("failed to stat telemetry file %s: %s", path.name, error)
            return

        if cursor is not None:
            if cursor.sealed:
                return
            if (
                stat_result.st_ino != cursor.inode
                or stat_result.st_dev != cursor.device
            ):
                self._record_warning(TelemetryWarningCode.SOURCE_FILE_CHANGED)
                self._file_cursors[str(path)] = _FileCursor(
                    path=str(path),
                    device=stat_result.st_dev,
                    inode=stat_result.st_ino,
                    offset=stat_result.st_size,
                    sealed=True,
                )
                return
            if stat_result.st_size < cursor.offset:
                self._record_warning(TelemetryWarningCode.SOURCE_FILE_CHANGED)
                self._file_cursors[str(path)] = _FileCursor(
                    path=str(path),
                    device=cursor.device,
                    inode=cursor.inode,
                    offset=stat_result.st_size,
                    sealed=True,
                )
                return

        start_offset = 0 if cursor is None else cursor.offset
        if stat_result.st_size == start_offset:
            if cursor is None:
                self._file_cursors[str(path)] = _FileCursor(
                    path=str(path),
                    device=stat_result.st_dev,
                    inode=stat_result.st_ino,
                    offset=start_offset,
                )
                self._dirty = True
            return

        try:
            raw_bytes = path.read_bytes()[start_offset:]
        except OSError as error:
            msg = f"failed to read telemetry file {path.name}: {error}"
            raise TelemetryProviderRuntimeError(msg) from error

        end_offset = self._consume_complete_records(
            raw_bytes=raw_bytes,
            path=path,
            start_offset=start_offset,
            file_size=stat_result.st_size,
            runtime_info=runtime_info,
        )

        self._file_cursors[str(path)] = _FileCursor(
            path=str(path),
            device=stat_result.st_dev,
            inode=stat_result.st_ino,
            offset=end_offset,
            sealed=bool(cursor and cursor.sealed),
        )
        self._dirty = True

    def _consume_complete_records(  # noqa: C901, PLR0911
        self,
        *,
        raw_bytes: bytes,
        path: Path,
        start_offset: int,
        file_size: int,
        runtime_info: TelemetryRuntimeInfo,
    ) -> int:
        if not raw_bytes:
            return start_offset

        newline_index = raw_bytes.rfind(b"\n")
        active_path = runtime_info.active_launch_path
        can_discard_partial = runtime_info.state == TelemetryRuntimeState.IDLE
        if runtime_info.state == TelemetryRuntimeState.RUNNING and active_path != str(
            path
        ):
            can_discard_partial = True

        if newline_index < 0:
            if len(raw_bytes) > self._limits.max_line_bytes:
                self._record_warning(TelemetryWarningCode.LINE_TOO_LONG)
                return file_size
            if can_discard_partial:
                self._record_warning(TelemetryWarningCode.PARTIAL_RECORD_DISCARDED)
                return file_size
            return start_offset

        complete_bytes = raw_bytes[: newline_index + 1]
        processed_bytes = 0
        for raw_line in complete_bytes.splitlines():
            line_length = len(raw_line)
            processed_bytes += line_length + 1
            if line_length > self._limits.max_line_bytes:
                self._record_warning(TelemetryWarningCode.LINE_TOO_LONG)
                continue
            self._process_line(
                path=path,
                offset=start_offset + processed_bytes - (line_length + 1),
                raw_line=raw_line,
            )

        end_offset = start_offset + processed_bytes
        trailing = raw_bytes[newline_index + 1 :]
        if trailing:
            if len(trailing) > self._limits.max_line_bytes:
                self._record_warning(TelemetryWarningCode.LINE_TOO_LONG)
                return file_size
            if can_discard_partial:
                self._record_warning(TelemetryWarningCode.PARTIAL_RECORD_DISCARDED)
                return file_size
        return end_offset

    def _process_line(  # noqa: C901, PLR0911
        self,
        *,
        path: Path,
        offset: int,
        raw_line: bytes,
    ) -> None:
        received_at = _isoformat_datetime(self._now())
        self._received_at = received_at
        try:
            payload = json.loads(raw_line)
        except json.JSONDecodeError:
            self._record_warning(TelemetryWarningCode.MALFORMED_RECORD)
            return

        record = _mapping(payload)
        if record is None:
            self._record_warning(TelemetryWarningCode.MALFORMED_RECORD)
            return

        record_type = _parse_string(record.get("type"))
        if record_type is None:
            self._record_warning(TelemetryWarningCode.MALFORMED_RECORD)
            return
        if record_type != "span":
            if record_type != "metric":
                self._record_warning(TelemetryWarningCode.UNKNOWN_RECORD)
            return

        span = self._normalize_span(record, path=path, offset=offset)
        if span is None:
            return

        encoded = span.key.encoded()
        existing = self._spans.get(encoded)
        if existing is None:
            if len(self._spans) >= self._limits.max_distinct_spans:
                self._record_warning(TelemetryWarningCode.SPAN_LIMIT_EXCEEDED)
                return
            self._spans[encoded] = span
            if span.source_version is not None:
                self._source_version = span.source_version
            self._observe_source_time(span.ended_at_ns)
            self._dirty = True
            return

        if existing.digest == span.digest:
            return

        self._duplicate_conflicts.append(
            _DuplicateConflict(
                key=span.key,
                first=existing.provenance,
                second=span.provenance,
            ),
        )
        self._record_warning(TelemetryWarningCode.DUPLICATE_CONFLICT)

    def _normalize_span(
        self,
        record: Mapping[str, object],
        *,
        path: Path,
        offset: int,
    ) -> _TelemetrySpan | None:
        trace_id = self._bounded_string(record.get("traceId"))
        span_id = self._bounded_string(record.get("spanId"))
        if trace_id is None or span_id is None:
            self._record_warning(TelemetryWarningCode.MALFORMED_RECORD)
            return None

        started_at_ns = _parse_timestamp_ns(record.get("startTime"))
        ended_at_ns = _parse_timestamp_ns(record.get("endTime"))
        if started_at_ns is None or ended_at_ns is None or ended_at_ns < started_at_ns:
            self._record_warning(TelemetryWarningCode.MALFORMED_RECORD)
            return None

        name = self._bounded_string(record.get("name"))
        if name is None:
            self._record_warning(TelemetryWarningCode.MALFORMED_RECORD)
            return None

        attributes = _mapping(record.get("attributes")) or {}
        events = _sequence_of_mappings(record.get("events"))
        if self._content_conflict(attributes, events):
            self._record_warning(TelemetryWarningCode.CONTENT_POLICY_CONFLICT)
            self._content_mode = TelemetryContentMode.POLICY_CONFLICT

        kind = _classify_span(name)
        provenance = _SpanProvenance(file_name=path.name, offset=offset)
        source_version = self._extract_source_version(record)
        usage: _ModelUsageDetails | None = None
        context: ContextUsage | None = None
        model = (
            self._bounded_string(
                attributes.get("gen_ai.response.model"),
            )
            or self._bounded_string(attributes.get("gen_ai.request.model"))
            or _span_suffix(
                name,
                "chat",
            )
        )
        requested_model = self._bounded_string(attributes.get("gen_ai.request.model"))
        provider = self._bounded_string(attributes.get("gen_ai.system"))
        conversation_id = self._bounded_string(attributes.get("gen_ai.conversation.id"))
        interaction_id = self._bounded_string(
            attributes.get("github.copilot.interaction_id"),
        )
        turn_id = self._bounded_string(attributes.get("github.copilot.turn_id"))
        response_id = self._bounded_string(attributes.get("gen_ai.response.id"))
        previous_response_id = self._bounded_string(
            attributes.get("gen_ai.request.previous_response.id"),
        )
        agent_id = self._bounded_string(attributes.get("gen_ai.agent.id"))
        agent_name = self._bounded_string(
            attributes.get("gen_ai.agent.name"),
        ) or _span_suffix(name, "invoke_agent")
        tool_name = self._bounded_string(
            attributes.get("gen_ai.tool.name"),
        ) or _span_suffix(name, "execute_tool")
        tool_type = self._bounded_string(attributes.get("gen_ai.tool.type"))
        tool_call_id = self._bounded_string(attributes.get("gen_ai.tool.call.id"))

        if kind == _SpanKind.MODEL_CALL:
            usage = self._normalize_model_usage(attributes)
            context = self._extract_context(events, fallback_time_ns=ended_at_ns)

        has_compaction = _events_contain_compaction(events)
        is_error = _status_is_error(record.get("status"))
        span = _TelemetrySpan(
            key=_SpanKey(trace_id=trace_id, span_id=span_id),
            parent_span_id=self._bounded_string(record.get("parentSpanId")),
            kind=kind,
            name=name,
            started_at_ns=started_at_ns,
            ended_at_ns=ended_at_ns,
            started_at=_isoformat_ns(started_at_ns),
            ended_at=_isoformat_ns(ended_at_ns),
            is_error=is_error,
            model=model,
            requested_model=requested_model,
            provider=provider,
            conversation_id=conversation_id,
            interaction_id=interaction_id,
            turn_id=turn_id,
            response_id=response_id,
            previous_response_id=previous_response_id,
            agent_id=agent_id,
            agent_name=agent_name,
            tool_name=tool_name,
            tool_type=tool_type,
            tool_call_id=tool_call_id,
            usage=usage,
            context=context,
            has_compaction_or_truncation=has_compaction,
            source_version=source_version,
            provenance=provenance,
            digest="",
        )
        digest = _hash_span(span)
        return replace(span, digest=digest)

    def _normalize_model_usage(
        self,
        attributes: Mapping[str, object],
    ) -> _ModelUsageDetails:
        raw_input_tokens = _parse_int(attributes.get("gen_ai.usage.input_tokens"))
        output_tokens = _parse_int(attributes.get("gen_ai.usage.output_tokens"))
        reasoning_output_tokens = _parse_int(
            attributes.get("gen_ai.usage.reasoning.output_tokens"),
        )
        cache_read_tokens = _parse_int(
            attributes.get("gen_ai.usage.cache_read.input_tokens"),
        )
        cache_write_tokens = _first_present_int(attributes, _CACHE_WRITE_ALIASES)

        cache_reporting = CacheReportingState.UNREPORTED
        if cache_read_tokens is not None or cache_write_tokens is not None:
            cache_reporting = CacheReportingState.REPORTED
            if cache_read_tokens is None:
                cache_read_tokens = 0
            if cache_write_tokens is None:
                cache_write_tokens = 0

        convention = self._resolve_accounting_convention(
            attributes=attributes,
            raw_input_tokens=raw_input_tokens,
            cache_reporting=cache_reporting,
            cache_read_tokens=cache_read_tokens,
            cache_write_tokens=cache_write_tokens,
        )

        effective_input_tokens: int | None = None
        other_input_tokens: int | None = None
        fresh_input_tokens: int | None = None
        total_tokens: int | None = None
        cache_reuse_percent: float | None = None

        if (
            raw_input_tokens is not None
            and convention == TokenAccountingConvention.INCLUSIVE
        ):
            effective_input_tokens = raw_input_tokens
            if (
                cache_reporting == CacheReportingState.REPORTED
                and cache_read_tokens is not None
                and cache_write_tokens is not None
            ):
                other = raw_input_tokens - cache_read_tokens - cache_write_tokens
                if other < 0:
                    self._record_warning(TelemetryWarningCode.INVALID_USAGE_SHAPE)
                else:
                    other_input_tokens = other
                    fresh_input_tokens = other + cache_write_tokens

        if (
            raw_input_tokens is not None
            and convention == TokenAccountingConvention.ADDITIVE
            and cache_reporting == CacheReportingState.REPORTED
            and cache_read_tokens is not None
            and cache_write_tokens is not None
        ):
            effective_input_tokens = (
                raw_input_tokens + cache_read_tokens + cache_write_tokens
            )
            other_input_tokens = raw_input_tokens
            fresh_input_tokens = raw_input_tokens + cache_write_tokens

        if effective_input_tokens is not None and output_tokens is not None:
            total_tokens = effective_input_tokens + output_tokens
        if (
            cache_reporting == CacheReportingState.REPORTED
            and effective_input_tokens is not None
            and effective_input_tokens > 0
            and cache_read_tokens is not None
        ):
            cache_reuse_percent = (cache_read_tokens / effective_input_tokens) * 100.0

        usage = UsageBreakdown(
            raw_input_tokens=raw_input_tokens,
            effective_input_tokens=effective_input_tokens,
            output_tokens=output_tokens,
            total_tokens=total_tokens,
            reasoning_output_tokens=reasoning_output_tokens,
            cache_read_input_tokens=cache_read_tokens
            if cache_reporting == CacheReportingState.REPORTED
            else None,
            cache_write_input_tokens=cache_write_tokens
            if cache_reporting == CacheReportingState.REPORTED
            else None,
            other_input_tokens=other_input_tokens,
            fresh_input_tokens=fresh_input_tokens,
            cache_reuse_percent=cache_reuse_percent,
            nano_aiu=_parse_int(attributes.get("github.copilot.nano_aiu")),
            opaque_cost=_parse_float(attributes.get("github.copilot.cost")),
        )
        return _ModelUsageDetails(
            usage=usage,
            cache_reporting=cache_reporting,
            token_accounting_convention=convention,
        )

    def _resolve_accounting_convention(
        self,
        *,
        attributes: Mapping[str, object],
        raw_input_tokens: int | None,
        cache_reporting: CacheReportingState,
        cache_read_tokens: int | None,
        cache_write_tokens: int | None,
    ) -> TokenAccountingConvention:
        explicit = _explicit_accounting_convention(attributes)
        candidate = explicit or self._default_token_accounting_convention
        if (
            raw_input_tokens is not None
            and cache_reporting == CacheReportingState.REPORTED
            and cache_read_tokens is not None
            and cache_write_tokens is not None
            and cache_read_tokens + cache_write_tokens > raw_input_tokens
        ):
            return TokenAccountingConvention.ADDITIVE
        return candidate or TokenAccountingConvention.UNKNOWN

    def _extract_context(
        self,
        events: tuple[Mapping[str, object], ...],
        *,
        fallback_time_ns: int,
    ) -> ContextUsage | None:
        best: tuple[int, ContextUsage] | None = None
        for event in events:
            attributes = _mapping(event.get("attributes")) or {}
            if not _contains_any_context_key(attributes):
                continue
            observed_ns = (
                _parse_timestamp_ns(
                    event.get("time"),
                )
                or _parse_timestamp_ns(event.get("timestamp"))
                or fallback_time_ns
            )
            context = ContextUsage(
                tokens=_parse_int(attributes.get("github.copilot.current_tokens")),
                limit=_parse_int(attributes.get("github.copilot.token_limit")),
                message_count=_parse_int(
                    attributes.get("github.copilot.messages_length"),
                ),
                observed_at=_isoformat_ns(observed_ns),
            )
            if best is None or observed_ns >= best[0]:
                best = (observed_ns, context)
        return None if best is None else best[1]

    def _extract_source_version(self, record: Mapping[str, object]) -> str | None:
        scope = _mapping(record.get("instrumentationScope")) or {}
        version = self._bounded_string(scope.get("version"))
        if version is not None:
            return version
        resource = _mapping(record.get("resource")) or {}
        resource_attributes = _mapping(resource.get("attributes")) or {}
        return self._bounded_string(
            resource_attributes.get("service.version"),
        ) or self._bounded_string(resource_attributes.get("github.copilot.version"))

    def _build_snapshot(  # noqa: C901, PLR0915
        self,
        runtime_info: TelemetryRuntimeInfo,
    ) -> TelemetrySnapshot:
        span_index = {span.key.encoded(): span for span in self._spans.values()}
        model_spans = sorted(
            [span for span in span_index.values() if span.kind == _SpanKind.MODEL_CALL],
            key=lambda span: (
                span.started_at_ns,
                span.ended_at_ns,
                span.key.trace_id,
                span.key.span_id,
            ),
        )
        tool_spans = [
            span for span in span_index.values() if span.kind == _SpanKind.TOOL_CALL
        ]
        agent_spans = [
            span for span in span_index.values() if span.kind == _SpanKind.AGENT
        ]

        subagent_ids = {
            span.key.encoded()
            for span in agent_spans
            if span.parent_span_id is not None
        }
        ancestor_cache: dict[str, _TelemetrySpan | None] = {}

        def nearest_agent(span: _TelemetrySpan) -> _TelemetrySpan | None:
            cached = ancestor_cache.get(span.key.encoded())
            if cached is not None or span.key.encoded() in ancestor_cache:
                return cached
            current_parent_id = span.parent_span_id
            while current_parent_id is not None:
                parent = span_index.get(f"{span.key.trace_id}:{current_parent_id}")
                if parent is None:
                    break
                if parent.kind == _SpanKind.AGENT:
                    ancestor_cache[span.key.encoded()] = parent
                    return parent
                current_parent_id = parent.parent_span_id
            ancestor_cache[span.key.encoded()] = None
            return None

        latest_call_span = model_spans[-1] if model_spans else None
        if model_spans:
            latest_call_span = max(
                model_spans,
                key=lambda span: (
                    span.started_at_ns,
                    span.ended_at_ns,
                    span.key.trace_id,
                    span.key.span_id,
                ),
            )

        subagent_model_spans = [
            span
            for span in model_spans
            if (
                (agent := nearest_agent(span)) is not None
                and agent.key.encoded() in subagent_ids
            )
        ]

        latest_context = None
        latest_context_ns = -1
        for span in model_spans:
            if span.context is None or span.context.observed_at is None:
                continue
            context_ns = _parse_isoformat_ns(span.context.observed_at)
            if context_ns is None:
                continue
            if context_ns >= latest_context_ns:
                latest_context = span.context
                latest_context_ns = context_ns

        session_usage = _aggregate_usage(
            [span.usage for span in model_spans if span.usage is not None],
        )
        subagent_usage = _aggregate_usage(
            [span.usage for span in subagent_model_spans if span.usage is not None],
        )

        reporting = ReportingCoverage(
            model_calls=len(model_spans),
            cache_reported_calls=sum(
                1
                for span in model_spans
                if span.usage is not None
                and span.usage.cache_reporting == CacheReportingState.REPORTED
            ),
            convention_resolved_calls=sum(
                1
                for span in model_spans
                if span.usage is not None
                and span.usage.token_accounting_convention
                != TokenAccountingConvention.UNKNOWN
            ),
            effective_input_covered_calls=sum(
                1
                for span in model_spans
                if span.usage is not None
                and span.usage.usage.effective_input_tokens is not None
            ),
            context_reported=latest_context is not None,
        )

        interaction_roots = [
            span for span in agent_spans if span.parent_span_id is None
        ]
        traces_with_spans = {span.key.trace_id for span in span_index.values()}
        traces_with_root = {span.key.trace_id for span in interaction_roots}
        interactions = len(interaction_roots) + len(
            traces_with_spans - traces_with_root
        )

        counts = ActivityCounts(
            interactions=interactions,
            model_calls=len(model_spans),
            tool_calls=len(tool_spans),
            subagent_invocations=len(subagent_ids),
            subagent_model_calls=len(subagent_model_spans),
            errors=sum(1 for span in span_index.values() if span.is_error),
        )

        subagent_duration_ms = _sum_all(
            _duration_ms(span.started_at_ns, span.ended_at_ns)
            for span in agent_spans
            if span.key.encoded() in subagent_ids
        )
        subagents = SubagentBreakdown(
            invocations=len(subagent_ids),
            model_calls=len(subagent_model_spans),
            effective_input_tokens=subagent_usage.effective_input_tokens,
            output_tokens=subagent_usage.output_tokens,
            cache_read_input_tokens=subagent_usage.cache_read_input_tokens,
            cache_write_input_tokens=subagent_usage.cache_write_input_tokens,
            duration_ms=subagent_duration_ms,
        )

        latest_call = None
        if latest_call_span is not None and latest_call_span.usage is not None:
            latest_agent = nearest_agent(latest_call_span)
            latest_call = ModelCallSummary(
                started_at=latest_call_span.started_at,
                ended_at=latest_call_span.ended_at,
                duration_ms=_duration_ms(
                    latest_call_span.started_at_ns,
                    latest_call_span.ended_at_ns,
                ),
                model=latest_call_span.model,
                requested_model=latest_call_span.requested_model,
                provider=latest_call_span.provider,
                agent_id=None if latest_agent is None else latest_agent.agent_id,
                agent_name=None if latest_agent is None else latest_agent.agent_name,
                is_subagent=(
                    latest_agent is not None
                    and latest_agent.key.encoded() in subagent_ids
                ),
                cache_reporting=latest_call_span.usage.cache_reporting,
                token_accounting_convention=(
                    latest_call_span.usage.token_accounting_convention
                ),
                usage=latest_call_span.usage.usage,
            )

        cache_signal = self._build_cache_signal(
            model_spans=model_spans,
            latest_call_span=latest_call_span,
            nearest_agent=nearest_agent,
        )
        warnings = TelemetryWarningSummary(
            total=sum(self._warning_counts.values()),
            items=tuple(
                TelemetryWarning(
                    code=TelemetryWarningCode(code),
                    count=count,
                )
                for code, count in sorted(self._warning_counts.items())
            ),
        )
        content_mode = self._content_mode
        observed_at = None
        if self._observed_at_ns is not None:
            observed_at = _isoformat_ns(self._observed_at_ns)

        state = TelemetryState.LIVE
        reason: str | None = None
        if not model_spans:
            if self._degraded_reasons:
                state = TelemetryState.DEGRADED
                reason = min(self._degraded_reasons)
            elif (
                runtime_info.state == TelemetryRuntimeState.RUNNING
                or self._has_source_files
            ):
                state = TelemetryState.STARTING
                reason = "waiting for first completed model call"
            else:
                state = TelemetryState.UNAVAILABLE
                reason = runtime_info.reason or "telemetry unavailable"
        elif self._degraded_reasons:
            state = TelemetryState.DEGRADED
            reason = min(self._degraded_reasons)

        return TelemetrySnapshot(
            schema_version=TELEMETRY_SNAPSHOT_SCHEMA_VERSION,
            state=state,
            reason=reason,
            content_mode=content_mode,
            source_version=self._source_version,
            observed_at=observed_at,
            received_at=self._received_at,
            session=session_usage,
            latest_call=latest_call,
            last_interaction=None,
            context=latest_context,
            counts=counts,
            subagents=subagents,
            cache_signal=cache_signal,
            reporting=reporting,
            warnings=warnings,
        )

    def _build_cache_signal(  # noqa: PLR0911
        self,
        *,
        model_spans: list[_TelemetrySpan],
        latest_call_span: _TelemetrySpan | None,
        nearest_agent: Callable[[_TelemetrySpan], _TelemetrySpan | None],
    ) -> CacheSignal | None:
        if latest_call_span is None or latest_call_span.usage is None:
            return None

        latest_agent = nearest_agent(latest_call_span)
        latest_lane = _call_lane(
            latest_call_span,
            latest_agent=latest_agent,
        )
        comparable: list[_TelemetrySpan] = [
            span
            for span in model_spans
            if span.usage is not None
            and _call_lane(span, latest_agent=nearest_agent(span)) == latest_lane
        ]
        if len(comparable) < 2:
            return CacheSignal(state=CacheSignalState.UNKNOWN)

        current = comparable[-1]
        previous = comparable[-2]
        if current.usage is None or previous.usage is None:
            return CacheSignal(state=CacheSignalState.UNKNOWN)
        current_model = current.model or current.requested_model
        previous_model = previous.model or previous.requested_model
        if current_model != previous_model and (
            current_model is not None or previous_model is not None
        ):
            return CacheSignal(
                state=CacheSignalState.EXPECTED_BOUNDARY,
                confidence=CacheSignalConfidence.LOW,
                reason=CacheSignalReason.MODEL_CHANGED,
            )

        current_usage = current.usage.usage
        previous_usage = previous.usage.usage
        if (
            current_usage.cache_reuse_percent is None
            or previous_usage.cache_reuse_percent is None
            or current_usage.fresh_input_tokens is None
            or current_usage.effective_input_tokens is None
        ):
            return CacheSignal(state=CacheSignalState.UNKNOWN)

        if current.has_compaction_or_truncation:
            return CacheSignal(
                state=CacheSignalState.CACHE_RESET_SUSPECTED,
                confidence=CacheSignalConfidence.MEDIUM,
                reason=CacheSignalReason.COMPACTION_OR_TRUNCATION,
            )

        current_fresh_share = (
            current_usage.fresh_input_tokens / current_usage.effective_input_tokens
            if current_usage.effective_input_tokens > 0
            else 0.0
        )
        if (
            previous_usage.cache_reuse_percent >= 50.0
            and current_usage.cache_reuse_percent < 10.0
            and current_fresh_share >= 0.5
        ):
            confidence = CacheSignalConfidence.LOW
            reason = CacheSignalReason.REUSE_COLLAPSED
            if (
                current.context is not None
                and previous.context is not None
                and current.context.tokens is not None
                and previous.context.tokens is not None
                and previous.context.tokens > 0
                and current.context.tokens <= previous.context.tokens // 2
            ):
                confidence = CacheSignalConfidence.MEDIUM
                reason = CacheSignalReason.CONTEXT_DISCONTINUITY
            return CacheSignal(
                state=CacheSignalState.CACHE_RESET_SUSPECTED,
                confidence=confidence,
                reason=reason,
            )

        return CacheSignal(state=CacheSignalState.HEALTHY)

    def _write_checkpoint(self, snapshot: TelemetrySnapshot) -> None:
        payload = {
            "checkpoint_version": TELEMETRY_CHECKPOINT_SCHEMA_VERSION,
            "content_mode": self._content_mode.value,
            "source_version": self._source_version,
            "received_at": self._received_at,
            "observed_at_ns": self._observed_at_ns,
            "has_source_files": self._has_source_files,
            "warnings": dict(self._warning_counts),
            "degraded_reasons": sorted(self._degraded_reasons),
            "files": [
                asdict(cursor)
                for cursor in sorted(
                    self._file_cursors.values(),
                    key=lambda cursor: cursor.path,
                )
            ],
            "duplicate_conflicts": [
                {
                    "key": asdict(conflict.key),
                    "first": asdict(conflict.first),
                    "second": asdict(conflict.second),
                }
                for conflict in self._duplicate_conflicts
            ],
            "spans": [
                _serialize_span(span)
                for span in sorted(
                    self._spans.values(),
                    key=lambda span: (
                        span.started_at_ns,
                        span.ended_at_ns,
                        span.key.trace_id,
                        span.key.span_id,
                    ),
                )
            ],
            "snapshot": asdict(snapshot),
        }
        _atomic_write_json(self._checkpoint_path, payload)

    def _observe_source_time(self, candidate_ns: int) -> None:
        if self._observed_at_ns is None or candidate_ns > self._observed_at_ns:
            self._observed_at_ns = candidate_ns

    def _record_warning(self, code: TelemetryWarningCode) -> None:
        self._warning_counts[code.value] += 1
        if code.value in _DEGRADED_WARNING_CODES:
            self._degraded_reasons.add(code.value)
        self._dirty = True

    def _bounded_string(self, value: object) -> str | None:
        parsed = _parse_string(value)
        if parsed is None:
            return None
        if len(parsed) <= self._limits.max_string_length:
            return parsed
        self._record_warning(TelemetryWarningCode.FIELD_TRUNCATED)
        return parsed[: self._limits.max_string_length]

    def _is_managed_jsonl_path(self, path: Path) -> bool:
        if not path.is_file() or path.suffix != ".jsonl":
            return False
        if path.parent != self._telemetry_dir:
            return False
        try:
            parsed = uuid.UUID(path.stem)
        except ValueError:
            return False
        return path.name == f"{parsed}.jsonl"

    def _content_conflict(
        self,
        attributes: Mapping[str, object],
        events: tuple[Mapping[str, object], ...],
    ) -> bool:
        for key, value in attributes.items():
            if key in _CONTENT_ATTRIBUTE_KEYS:
                return True
            if key == "gen_ai.tool.definitions" and not _tool_definitions_are_safe(
                value,
                max_items=self._limits.max_tool_definitions,
            ):
                return True
        for event in events:
            event_attributes = _mapping(event.get("attributes")) or {}
            for key, value in event_attributes.items():
                if key in _CONTENT_ATTRIBUTE_KEYS:
                    return True
                if key == "gen_ai.tool.definitions" and not _tool_definitions_are_safe(
                    value,
                    max_items=self._limits.max_tool_definitions,
                ):
                    return True
        return False


def unavailable_snapshot(reason: str) -> TelemetrySnapshot:
    return TelemetrySnapshot(
        schema_version=TELEMETRY_SNAPSHOT_SCHEMA_VERSION,
        state=TelemetryState.UNAVAILABLE,
        reason=reason,
    )


def telemetry_provider_from_env(
    *,
    runtime_info_provider: Callable[[], Awaitable[TelemetryRuntimeInfo]],
    env: Mapping[str, str] | None = None,
) -> SessionTelemetryProvider:
    source = dict(os.environ if env is None else env)
    harness = source.get("KERNEL_HARNESS")
    if harness != "copilot-cli":
        return UnavailableSessionTelemetryProvider(
            f"telemetry is unavailable for harness {harness or 'unknown'}",
        )

    default_convention = TokenAccountingConvention.INCLUSIVE
    if source.get("CONNECTION_URL"):
        default_convention = TokenAccountingConvention.UNKNOWN

    return CopilotOtelTelemetryProvider(
        telemetry_dir=str(_MANAGED_TELEMETRY_DIR),
        runtime_info_provider=runtime_info_provider,
        default_token_accounting_convention=default_convention,
    )


def _utc_now() -> datetime:
    return datetime.now(UTC)


def _isoformat_datetime(value: datetime) -> str:
    return value.astimezone(UTC).isoformat().replace("+00:00", "Z")


def _isoformat_ns(value: int) -> str:
    seconds, nanos = divmod(value, 1_000_000_000)
    base = datetime.fromtimestamp(seconds, UTC).strftime("%Y-%m-%dT%H:%M:%S")
    if nanos:
        return f"{base}.{nanos:09d}Z"
    return f"{base}Z"


def _parse_isoformat_ns(value: str) -> int | None:
    if not value.endswith("Z"):
        return None
    trimmed = value[:-1]
    if "." in trimmed:
        prefix, fraction = trimmed.split(".", 1)
        if len(fraction) > 9 or not fraction.isdigit():
            return None
        fraction_ns = int(fraction.ljust(9, "0"))
    else:
        prefix = trimmed
        fraction_ns = 0
    try:
        base = datetime.fromisoformat(prefix)
    except ValueError:
        return None
    return int(base.replace(tzinfo=UTC).timestamp()) * 1_000_000_000 + fraction_ns


def _mapping(value: object) -> dict[str, object] | None:
    if not isinstance(value, dict):
        return None
    raw_mapping = cast("dict[object, object]", value)
    result: dict[str, object] = {}
    for key, item in raw_mapping.items():
        if not isinstance(key, str):
            return None
        result[key] = item
    return result


def _sequence(value: object) -> tuple[object, ...] | None:
    if isinstance(value, (str, bytes, bytearray)):
        return None
    if isinstance(value, list):
        return tuple(cast("list[object]", value))
    if not isinstance(value, tuple):
        return None
    return cast("tuple[object, ...]", value)


def _sequence_of_mappings(value: object) -> tuple[Mapping[str, object], ...]:
    sequence = _sequence(value)
    if sequence is None:
        return ()
    items: list[Mapping[str, object]] = []
    for item in sequence:
        mapping = _mapping(item)
        if mapping is not None:
            items.append(mapping)
    return tuple(items)


def _string_sequence(value: object) -> tuple[str, ...] | None:
    sequence = _sequence(value)
    if sequence is None:
        return None
    items: list[str] = []
    for item in sequence:
        parsed = _parse_string(item)
        if parsed is None:
            return None
        items.append(parsed)
    return tuple(items)


def _parse_string(value: object) -> str | None:
    if isinstance(value, str):
        return value
    return None


def _parse_int(value: object) -> int | None:
    if isinstance(value, bool):
        return None
    parsed: int | None = None
    if isinstance(value, int):
        parsed = value
    elif isinstance(value, float):
        if value.is_integer():
            parsed = int(value)
    elif isinstance(value, str):
        try:
            parsed = int(value)
        except ValueError:
            parsed = None
    return parsed


def _parse_float(value: object) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        try:
            return float(value)
        except ValueError:
            return None
    return None


def _parse_timestamp_ns(value: object) -> int | None:
    sequence = _sequence(value)
    if sequence is None or len(sequence) != 2:
        return None
    seconds = _parse_int(sequence[0])
    nanos = _parse_int(sequence[1])
    if seconds is None or nanos is None or nanos < 0 or nanos >= 1_000_000_000:
        return None
    return seconds * 1_000_000_000 + nanos


def _classify_span(name: str) -> _SpanKind:
    if name == "invoke_agent" or name.startswith("invoke_agent "):
        return _SpanKind.AGENT
    if name == "chat" or name.startswith("chat "):
        return _SpanKind.MODEL_CALL
    if name == "execute_tool" or name.startswith("execute_tool "):
        return _SpanKind.TOOL_CALL
    return _SpanKind.OTHER


def _span_suffix(name: str, prefix: str) -> str | None:
    if name == prefix:
        return None
    if name.startswith(f"{prefix} "):
        return name[len(prefix) + 1 :]
    return None


def _first_present_int(
    attributes: Mapping[str, object], keys: Sequence[str]
) -> int | None:
    for key in keys:
        value = _parse_int(attributes.get(key))
        if value is not None:
            return value
    return None


def _explicit_accounting_convention(
    attributes: Mapping[str, object],
) -> TokenAccountingConvention | None:
    for key in _EXPLICIT_ACCOUNTING_KEYS:
        raw = _parse_string(attributes.get(key))
        if raw is None:
            continue
        try:
            return TokenAccountingConvention(raw)
        except ValueError:
            continue
    return None


def _contains_any_context_key(attributes: Mapping[str, object]) -> bool:
    return any(
        key in attributes
        for key in (
            "github.copilot.current_tokens",
            "github.copilot.token_limit",
            "github.copilot.messages_length",
        )
    )


def _events_contain_compaction(events: Sequence[Mapping[str, object]]) -> bool:
    for event in events:
        name = _parse_string(event.get("name"))
        if name is not None and any(
            token in name.lower() for token in _TRUNCATION_EVENT_TOKENS
        ):
            return True
        attributes = _mapping(event.get("attributes")) or {}
        for key in attributes:
            lowered = key.lower()
            if any(token in lowered for token in _TRUNCATION_EVENT_TOKENS):
                return True
    return False


def _status_is_error(value: object) -> bool:
    mapping = _mapping(value)
    if mapping is None:
        return False
    code = mapping.get("code")
    if isinstance(code, str):
        return code.upper() == "ERROR"
    if isinstance(code, int):
        return code != 0
    return False


def _hash_span(span: _TelemetrySpan) -> str:
    payload = _serialize_span(replace(span, digest=""))
    payload.pop("digest", None)
    payload.pop("provenance", None)
    encoded = json.dumps(
        payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    )
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def _serialize_span(span: _TelemetrySpan) -> dict[str, object]:
    usage: dict[str, object] | None = None
    if span.usage is not None:
        usage = {
            "usage": asdict(span.usage.usage),
            "cache_reporting": span.usage.cache_reporting.value,
            "token_accounting_convention": span.usage.token_accounting_convention.value,
        }
    return {
        "key": asdict(span.key),
        "parent_span_id": span.parent_span_id,
        "kind": span.kind.value,
        "name": span.name,
        "started_at_ns": span.started_at_ns,
        "ended_at_ns": span.ended_at_ns,
        "started_at": span.started_at,
        "ended_at": span.ended_at,
        "is_error": span.is_error,
        "model": span.model,
        "requested_model": span.requested_model,
        "provider": span.provider,
        "conversation_id": span.conversation_id,
        "interaction_id": span.interaction_id,
        "turn_id": span.turn_id,
        "response_id": span.response_id,
        "previous_response_id": span.previous_response_id,
        "agent_id": span.agent_id,
        "agent_name": span.agent_name,
        "tool_name": span.tool_name,
        "tool_type": span.tool_type,
        "tool_call_id": span.tool_call_id,
        "usage": usage,
        "context": None if span.context is None else asdict(span.context),
        "has_compaction_or_truncation": span.has_compaction_or_truncation,
        "source_version": span.source_version,
        "provenance": asdict(span.provenance),
        "digest": span.digest,
    }


def _deserialize_file_cursor(value: Mapping[str, object]) -> _FileCursor | None:
    path = _parse_string(value.get("path"))
    device = _parse_int(value.get("device"))
    inode = _parse_int(value.get("inode"))
    offset = _parse_int(value.get("offset"))
    sealed = value.get("sealed")
    if (
        path is None
        or device is None
        or inode is None
        or offset is None
        or not isinstance(sealed, bool)
    ):
        return None
    return _FileCursor(
        path=path,
        device=device,
        inode=inode,
        offset=offset,
        sealed=sealed,
    )


def _deserialize_duplicate_conflict(
    value: Mapping[str, object],
) -> _DuplicateConflict | None:
    key = _mapping(value.get("key"))
    first = _mapping(value.get("first"))
    second = _mapping(value.get("second"))
    if key is None or first is None or second is None:
        return None
    span_key = _deserialize_span_key(key)
    first_provenance = _deserialize_span_provenance(first)
    second_provenance = _deserialize_span_provenance(second)
    if span_key is None or first_provenance is None or second_provenance is None:
        return None
    return _DuplicateConflict(
        key=span_key,
        first=first_provenance,
        second=second_provenance,
    )


def _deserialize_span(  # noqa: C901, PLR0911
    value: Mapping[str, object],
) -> _TelemetrySpan | None:
    key = _deserialize_span_key(_mapping(value.get("key")) or {})
    provenance = _deserialize_span_provenance(_mapping(value.get("provenance")) or {})
    kind_raw = _parse_string(value.get("kind"))
    if key is None or provenance is None or kind_raw is None:
        return None
    try:
        kind = _SpanKind(kind_raw)
    except ValueError:
        return None

    started_at_ns = _parse_int(value.get("started_at_ns"))
    ended_at_ns = _parse_int(value.get("ended_at_ns"))
    started_at = _parse_string(value.get("started_at"))
    ended_at = _parse_string(value.get("ended_at"))
    is_error = value.get("is_error")
    digest = _parse_string(value.get("digest"))
    if (
        started_at_ns is None
        or ended_at_ns is None
        or started_at is None
        or ended_at is None
        or not isinstance(is_error, bool)
        or digest is None
    ):
        return None

    usage_payload = value.get("usage")
    usage: _ModelUsageDetails | None = None
    if usage_payload is not None:
        usage_mapping = _mapping(usage_payload)
        if usage_mapping is None:
            return None
        usage = _deserialize_model_usage(usage_mapping)
        if usage is None:
            return None

    context_payload = value.get("context")
    context = None
    if context_payload is not None:
        context_mapping = _mapping(context_payload)
        if context_mapping is None:
            return None
        context = _deserialize_context(context_mapping)
        if context is None:
            return None

    has_compaction_or_truncation = value.get("has_compaction_or_truncation")
    if not isinstance(has_compaction_or_truncation, bool):
        return None

    return _TelemetrySpan(
        key=key,
        parent_span_id=_parse_string(value.get("parent_span_id")),
        kind=kind,
        name=_parse_string(value.get("name")),
        started_at_ns=started_at_ns,
        ended_at_ns=ended_at_ns,
        started_at=started_at,
        ended_at=ended_at,
        is_error=is_error,
        model=_parse_string(value.get("model")),
        requested_model=_parse_string(value.get("requested_model")),
        provider=_parse_string(value.get("provider")),
        conversation_id=_parse_string(value.get("conversation_id")),
        interaction_id=_parse_string(value.get("interaction_id")),
        turn_id=_parse_string(value.get("turn_id")),
        response_id=_parse_string(value.get("response_id")),
        previous_response_id=_parse_string(value.get("previous_response_id")),
        agent_id=_parse_string(value.get("agent_id")),
        agent_name=_parse_string(value.get("agent_name")),
        tool_name=_parse_string(value.get("tool_name")),
        tool_type=_parse_string(value.get("tool_type")),
        tool_call_id=_parse_string(value.get("tool_call_id")),
        usage=usage,
        context=context,
        has_compaction_or_truncation=has_compaction_or_truncation,
        source_version=_parse_string(value.get("source_version")),
        provenance=provenance,
        digest=digest,
    )


def _deserialize_span_key(value: Mapping[str, object]) -> _SpanKey | None:
    trace_id = _parse_string(value.get("trace_id"))
    span_id = _parse_string(value.get("span_id"))
    if trace_id is None or span_id is None:
        return None
    return _SpanKey(trace_id=trace_id, span_id=span_id)


def _deserialize_span_provenance(value: Mapping[str, object]) -> _SpanProvenance | None:
    file_name = _parse_string(value.get("file_name"))
    offset = _parse_int(value.get("offset"))
    if file_name is None or offset is None:
        return None
    return _SpanProvenance(file_name=file_name, offset=offset)


def _deserialize_model_usage(value: Mapping[str, object]) -> _ModelUsageDetails | None:
    usage_mapping = _mapping(value.get("usage"))
    cache_reporting = _parse_string(value.get("cache_reporting"))
    convention = _parse_string(value.get("token_accounting_convention"))
    if usage_mapping is None or cache_reporting is None or convention is None:
        return None
    usage = _deserialize_usage(usage_mapping)
    if usage is None:
        return None
    try:
        return _ModelUsageDetails(
            usage=usage,
            cache_reporting=CacheReportingState(cache_reporting),
            token_accounting_convention=TokenAccountingConvention(convention),
        )
    except ValueError:
        return None


def _deserialize_usage(value: Mapping[str, object]) -> UsageBreakdown | None:
    return UsageBreakdown(
        raw_input_tokens=_parse_int(value.get("raw_input_tokens")),
        effective_input_tokens=_parse_int(value.get("effective_input_tokens")),
        output_tokens=_parse_int(value.get("output_tokens")),
        total_tokens=_parse_int(value.get("total_tokens")),
        reasoning_output_tokens=_parse_int(value.get("reasoning_output_tokens")),
        cache_read_input_tokens=_parse_int(value.get("cache_read_input_tokens")),
        cache_write_input_tokens=_parse_int(value.get("cache_write_input_tokens")),
        other_input_tokens=_parse_int(value.get("other_input_tokens")),
        fresh_input_tokens=_parse_int(value.get("fresh_input_tokens")),
        cache_reuse_percent=_parse_float(value.get("cache_reuse_percent")),
        nano_aiu=_parse_int(value.get("nano_aiu")),
        opaque_cost=_parse_float(value.get("opaque_cost")),
    )


def _deserialize_context(value: Mapping[str, object]) -> ContextUsage | None:
    observed_at = value.get("observed_at")
    if observed_at is not None and not isinstance(observed_at, str):
        return None
    return ContextUsage(
        tokens=_parse_int(value.get("tokens")),
        limit=_parse_int(value.get("limit")),
        message_count=_parse_int(value.get("message_count")),
        observed_at=observed_at,
    )


def _aggregate_usage(usages: Sequence[_ModelUsageDetails]) -> UsageBreakdown:
    if not usages:
        return UsageBreakdown()
    breakdowns = [usage.usage for usage in usages]
    effective_inputs = [usage.effective_input_tokens for usage in breakdowns]
    cache_reads = [usage.cache_read_input_tokens for usage in breakdowns]
    effective_sum = _sum_all(effective_inputs)
    cache_read_sum = _sum_all(cache_reads)
    cache_reuse_percent = None
    if (
        effective_sum is not None
        and cache_read_sum is not None
        and effective_sum > 0
        and all(
            usage.cache_reporting == CacheReportingState.REPORTED for usage in usages
        )
    ):
        cache_reuse_percent = (cache_read_sum / effective_sum) * 100.0

    return UsageBreakdown(
        raw_input_tokens=_sum_all(
            breakdown.raw_input_tokens for breakdown in breakdowns
        ),
        effective_input_tokens=effective_sum,
        output_tokens=_sum_all(breakdown.output_tokens for breakdown in breakdowns),
        total_tokens=_sum_all(breakdown.total_tokens for breakdown in breakdowns),
        reasoning_output_tokens=_sum_all(
            breakdown.reasoning_output_tokens for breakdown in breakdowns
        ),
        cache_read_input_tokens=cache_read_sum,
        cache_write_input_tokens=_sum_all(
            breakdown.cache_write_input_tokens for breakdown in breakdowns
        ),
        other_input_tokens=_sum_all(
            breakdown.other_input_tokens for breakdown in breakdowns
        ),
        fresh_input_tokens=_sum_all(
            breakdown.fresh_input_tokens for breakdown in breakdowns
        ),
        cache_reuse_percent=cache_reuse_percent,
        nano_aiu=_sum_all(breakdown.nano_aiu for breakdown in breakdowns),
        opaque_cost=_sum_all_float(breakdown.opaque_cost for breakdown in breakdowns),
    )


def _sum_all(values: Iterable[int | None] | Iterable[int]) -> int | None:
    total = 0
    for value in values:
        if value is None:
            return None
        total += int(value)
    return total


def _sum_all_float(values: Iterable[float | None] | Iterable[float]) -> float | None:
    total = 0.0
    for value in values:
        if value is None:
            return None
        total += float(value)
    return total


def _duration_ms(started_at_ns: int, ended_at_ns: int) -> int:
    return max(0, (ended_at_ns - started_at_ns) // 1_000_000)


def _call_lane(
    span: _TelemetrySpan,
    *,
    latest_agent: _TelemetrySpan | None,
) -> tuple[str | None, str | None, str | None]:
    agent_identity = None
    if latest_agent is not None:
        agent_identity = (
            latest_agent.agent_id
            or latest_agent.agent_name
            or latest_agent.key.encoded()
        )
    return (
        span.conversation_id or span.key.trace_id,
        agent_identity,
        span.model or span.requested_model,
    )


def _tool_definitions_are_safe(  # noqa: C901, PLR0911, PLR0912
    value: object,
    *,
    max_items: int,
) -> bool:
    if value is None:
        return True
    parsed: object = value
    if isinstance(value, str):
        try:
            parsed = json.loads(value)
        except json.JSONDecodeError:
            return False
    if not isinstance(parsed, list):
        return False
    parsed_list = cast("list[object]", parsed)
    if len(parsed_list) > max_items:
        return False
    for item in parsed_list:
        if not isinstance(item, dict):
            return False
        raw_item = cast("dict[object, object]", item)
        keys: set[str] = set()
        for key in raw_item:
            if not isinstance(key, str):
                return False
            keys.add(key)
        if not keys <= _ALLOWED_TOOL_DEFINITION_KEYS:
            if keys & _CONTENT_TOOL_DEFINITION_KEYS:
                return False
            return False
        for candidate in raw_item.values():
            if candidate is not None and not isinstance(candidate, str):
                return False
    return True


def _atomic_write_json(path: Path, payload: Mapping[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary_path = path.with_name(f"{path.name}.{uuid.uuid4()}.tmp")
    try:
        with temporary_path.open("w", encoding="utf-8") as handle:
            json.dump(payload, handle, sort_keys=True, separators=(",", ":"))
            handle.flush()
            os.fsync(handle.fileno())
        temporary_path.replace(path)
        try:
            directory_fd = os.open(path.parent, os.O_RDONLY)
        except OSError:
            return
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if temporary_path.exists():
            with suppress(OSError):
                temporary_path.unlink()
