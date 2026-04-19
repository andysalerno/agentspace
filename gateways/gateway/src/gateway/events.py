"""Gateway audit events — used by gateways to record activity for logs/UI."""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from datetime import UTC, datetime
from enum import StrEnum


class GatewayEventType(StrEnum):
    INBOUND = "inbound"
    OUTBOUND = "outbound"
    ERROR = "error"
    STATUS = "status"


def utc_now_iso() -> str:
    return datetime.now(UTC).isoformat()


@dataclass(frozen=True, slots=True)
class GatewayEvent:
    type: GatewayEventType
    ts: str = field(default_factory=utc_now_iso)
    sender: str | None = None
    content: str | None = None
    session_id: str | None = None
    message: str | None = None

    def to_dict(self) -> dict[str, object]:
        return {key: value for key, value in asdict(self).items() if value is not None}
