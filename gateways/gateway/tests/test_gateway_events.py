from __future__ import annotations

from gateway.events import GatewayEvent, GatewayEventType
from gateway.protocol import GatewayStatus, GatewayType


def test_event_to_dict_omits_none_fields() -> None:
    event = GatewayEvent(type=GatewayEventType.INBOUND, sender="alice", content="hi")

    payload = event.to_dict()

    assert payload["type"] == "inbound"
    assert payload["sender"] == "alice"
    assert payload["content"] == "hi"
    assert "session_id" not in payload
    assert "message" not in payload
    assert "ts" in payload


def test_status_and_type_string_values() -> None:
    assert GatewayStatus.RUNNING.value == "running"
    assert GatewayType.ECHO.value == "echo"
