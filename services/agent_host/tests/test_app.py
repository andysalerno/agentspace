from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from agent_host.app import app
from agent_host.service import AgentHost
from fastapi.testclient import TestClient
from kernel.events import (
    KernelEvent,
    KernelStatus,
    session_end,
    session_start,
    status_event,
    text_delta,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from kernel.protocol import KernelConfig


class StubKernel:
    def __init__(self) -> None:
        self.resume_token_value: str | None = None

    @property
    def name(self) -> str:
        return "stub"

    @property
    def status(self) -> KernelStatus:
        return KernelStatus.DONE

    @property
    def resume_token(self) -> str | None:
        return self.resume_token_value

    async def start(self, config: KernelConfig) -> None:
        del config

    async def send(self, message: str) -> None:
        del message
        if self.resume_token_value is None:
            self.resume_token_value = "resume-ref-app"  # noqa: S105

    async def recv(self) -> AsyncIterator[KernelEvent]:
        yield session_start("kernel-session", "stub")
        yield status_event(KernelStatus.BUSY)
        yield text_delta("ok")
        yield status_event(KernelStatus.DONE)
        yield session_end()

    async def stop(self) -> None:
        return


@pytest.fixture
def client(monkeypatch: pytest.MonkeyPatch) -> TestClient:
    host = AgentHost()

    def fake_get_kernel(_harness_name: str) -> StubKernel:
        return StubKernel()

    monkeypatch.setattr("agent_host.app.host", host)
    monkeypatch.setattr("agent_host.service.get_kernel", fake_get_kernel)
    return TestClient(app)


def test_healthz(client: TestClient) -> None:
    response = client.get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"status": "ok"}


def test_session_lifecycle(client: TestClient) -> None:
    created = client.post("/sessions", json={"harness": "copilot-cli"})
    session_id = created.json()["session_id"]

    message = client.post(
        f"/sessions/{session_id}/messages",
        json={"message": "hello"},
    )
    history = client.get(f"/sessions/{session_id}/history")
    session = client.get(f"/sessions/{session_id}")
    destroyed = client.delete(f"/sessions/{session_id}")

    assert created.status_code == 200
    assert message.status_code == 200
    assert history.status_code == 200
    assert session.status_code == 200
    assert destroyed.status_code == 204
    assert message.json()["events"][2]["content"] == "ok"
    assert history.json()["history"][0][2]["content"] == "ok"
    assert session.json()["resume_token"] == "resume-ref-app"  # noqa: S105
