from __future__ import annotations

import importlib
from typing import TYPE_CHECKING

import pytest
from agent_host.service import AgentHost, KernelRuntimeSession
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
    from kernel_host.registry import HarnessName


class StubRuntime:
    def __init__(self) -> None:
        self._summaries: dict[str, dict[str, object]] = {}
        self._histories: dict[str, list[list[KernelEvent]]] = {}

    async def create_session(
        self,
        *,
        session_id: str,
        harness: HarnessName,
        env: dict[str, str],
        cwd: str | None,
        additional_paths: tuple[str, ...],
    ) -> KernelRuntimeSession:
        del harness, env, cwd, additional_paths
        container_name = f"container-{session_id[:8]}"
        self._summaries[container_name] = {
            "status": "idle",
            "resume_token": "resume-runtime-1",
        }
        self._histories[container_name] = []
        return KernelRuntimeSession(value=container_name)

    async def send_message(
        self,
        *,
        session: KernelRuntimeSession,
        message: str,
    ) -> list[KernelEvent]:
        container_name = self._session_key(session)
        events = [
            session_start("kernel-session", "stub"),
            status_event(KernelStatus.BUSY),
            text_delta(message),
            status_event(KernelStatus.DONE),
            session_end(),
        ]
        self._histories[container_name].append(events)
        self._summaries[container_name] = {
            "status": "done",
            "resume_token": "resume-runtime-2",
        }
        return events

    async def summary(self, *, session: KernelRuntimeSession) -> dict[str, object]:
        return dict(self._summaries[self._session_key(session)])

    async def history(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> list[list[KernelEvent]]:
        return list(self._histories[self._session_key(session)])

    async def destroy_session(self, *, session: KernelRuntimeSession) -> None:
        del session

    def _session_key(self, session: KernelRuntimeSession) -> str:
        assert isinstance(session.value, str)
        return session.value


@pytest.fixture
def client(monkeypatch: pytest.MonkeyPatch) -> TestClient:
    agent_host_app = importlib.import_module("agent_host.app")
    host = AgentHost(runtime=StubRuntime())
    monkeypatch.setattr(agent_host_app, "host", host)
    return TestClient(agent_host_app.app)


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
    assert message.json()["events"][2]["content"] == "hello"
    assert history.json()["history"][0][2]["content"] == "hello"
    assert session.json()["resume_token"].startswith("resume-runtime-")
