"""Standard kernel event types.

All kernels emit these events as JSONL on stdout. This is the contract
that kernel_host (and eventually agent-host) consumes.
"""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from datetime import UTC, datetime
from enum import StrEnum
from typing import Any


class EventType(StrEnum):
    SESSION_START = "session_start"
    STATUS = "status"
    TEXT_DELTA = "text_delta"
    REASONING_DELTA = "reasoning_delta"
    TOOL_CALL = "tool_call"
    TOOL_RESULT = "tool_result"
    ERROR = "error"
    SESSION_END = "session_end"


class KernelStatus(StrEnum):
    IDLE = "idle"
    BUSY = "busy"
    ERROR = "error"
    DONE = "done"


def _now() -> str:
    return datetime.now(UTC).isoformat()


@dataclass(frozen=True, slots=True)
class KernelEvent:
    type: EventType
    ts: str = field(default_factory=_now)
    session_id: str | None = None
    kernel: str | None = None
    status: KernelStatus | None = None
    content: str | None = None
    tool: str | None = None
    input: dict[str, Any] | None = None
    output: str | None = None
    message: str | None = None

    def to_jsonl(self) -> str:
        d = {k: v for k, v in asdict(self).items() if v is not None}
        return json.dumps(d, separators=(",", ":"))


def session_start(session_id: str, kernel_name: str) -> KernelEvent:
    return KernelEvent(
        type=EventType.SESSION_START,
        session_id=session_id,
        kernel=kernel_name,
    )


def status_event(status: KernelStatus) -> KernelEvent:
    return KernelEvent(type=EventType.STATUS, status=status)


def text_delta(content: str) -> KernelEvent:
    return KernelEvent(type=EventType.TEXT_DELTA, content=content)


def reasoning_delta(content: str) -> KernelEvent:
    return KernelEvent(type=EventType.REASONING_DELTA, content=content)


def tool_call(tool: str, tool_input: dict[str, Any]) -> KernelEvent:
    return KernelEvent(type=EventType.TOOL_CALL, tool=tool, input=tool_input)


def tool_result(tool: str, tool_output: str) -> KernelEvent:
    return KernelEvent(type=EventType.TOOL_RESULT, tool=tool, output=tool_output)


def error(message: str) -> KernelEvent:
    return KernelEvent(type=EventType.ERROR, message=message)


def session_end() -> KernelEvent:
    return KernelEvent(type=EventType.SESSION_END)
