from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, cast

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient
from gateway.protocol import GatewayConfig, GatewayStatus
from gateway_echo import EchoGateway

if TYPE_CHECKING:
    from gateway.client import ClientServiceClient


@dataclass
class FakeClient:
    sessions: dict[str, str] = field(default_factory=dict[str, str])
    sent: list[tuple[str, str]] = field(default_factory=list[tuple[str, str]])
    next_session_counter: int = 0

    async def create_session(
        self,
        *,
        agent_id: str,
        channel_name: str | None = None,
    ) -> dict[str, object]:
        del agent_id, channel_name
        self.next_session_counter += 1
        session_id = f"session-{self.next_session_counter}"
        return {"session_id": session_id}

    async def send_message(
        self,
        *,
        session_id: str,
        message: str,
    ) -> dict[str, object]:
        self.sent.append((session_id, message))
        return {
            "assistant_message": {
                "content": f"echo: {message}",
            },
        }

    async def delete_session(self, *, session_id: str) -> None:
        del session_id


def _make_app(gateway: EchoGateway) -> FastAPI:
    app = FastAPI()
    router = gateway.extra_router()
    assert router is not None
    app.include_router(router)
    return app


def _make_config(client: FakeClient) -> GatewayConfig:
    return GatewayConfig(
        gateway_id="gw-1",
        agent_id="agent-1",
        client=cast("ClientServiceClient", client),
        env={},
    )


@pytest.mark.asyncio
async def test_echo_gateway_lifecycle_and_inbox() -> None:
    gateway = EchoGateway()
    fake = FakeClient()
    config = _make_config(fake)

    await gateway.start(config)
    assert gateway.status is GatewayStatus.RUNNING

    app = _make_app(gateway)
    client = TestClient(app)

    response = client.post(
        "/gateway/inbox",
        json={"sender": "alice", "text": "hello"},
    )
    assert response.status_code == 200
    body = response.json()
    assert body["sender"] == "alice"
    assert body["text"] == "hello"
    assert body["reply"] == "echo: hello"
    assert body["session_id"] == "session-1"

    again = client.post(
        "/gateway/inbox",
        json={"sender": "alice", "text": "again"},
    )
    assert again.status_code == 200
    assert again.json()["session_id"] == "session-1"

    other = client.post(
        "/gateway/inbox",
        json={"sender": "bob", "text": "hey"},
    )
    assert other.status_code == 200
    assert other.json()["session_id"] == "session-2"

    outbox = client.get("/gateway/outbox").json()["entries"]
    assert [entry["sender"] for entry in outbox] == ["alice", "alice", "bob"]

    events = client.get("/gateway/events").json()["events"]
    types = [event["type"] for event in events]
    assert "inbound" in types
    assert "outbound" in types

    await gateway.stop()
    assert gateway.status is GatewayStatus.STOPPED


@pytest.mark.asyncio
async def test_inbox_returns_503_when_not_running() -> None:
    gateway = EchoGateway()
    app = _make_app(gateway)
    client = TestClient(app)

    response = client.post(
        "/gateway/inbox",
        json={"sender": "alice", "text": "hi"},
    )
    assert response.status_code == 503
