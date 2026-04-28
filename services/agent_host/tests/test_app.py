from __future__ import annotations

import importlib
import json
from typing import TYPE_CHECKING

import pytest
from agent_host.service import AgentHost, KernelRuntimeSession
from agent_host.skills import SkillsService
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
        additional_paths: tuple[str, ...],
        skills: tuple[str, ...] = (),
    ) -> KernelRuntimeSession:
        del harness, env, additional_paths, skills
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
        return [
            event
            async for event in self.stream_message(
                session=session,
                message=message,
            )
        ]

    def stream_message(
        self,
        *,
        session: KernelRuntimeSession,
        message: str,
    ) -> AsyncIterator[KernelEvent]:
        container_name = self._session_key(session)
        events = [
            session_start("kernel-session", "stub"),
            status_event(KernelStatus.BUSY),
            text_delta(message),
            status_event(KernelStatus.DONE),
            session_end(),
        ]

        async def iterator() -> AsyncIterator[KernelEvent]:
            for event in events:
                yield event
            self._histories[container_name].append(events)
            self._summaries[container_name] = {
                "status": "done",
                "resume_token": "resume-runtime-2",
            }

        return iterator()

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

    async def logs(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> list[str]:
        del session
        return ['{"type":"stub","data":{}}']

    async def container_logs(
        self,
        *,
        session: KernelRuntimeSession,
        tail: int | None,
    ) -> list[str]:
        container_name = self._session_key(session)
        lines = [f"{container_name} container line {i}" for i in range(5)]
        if tail is not None and tail > 0:
            return lines[-tail:]
        return lines

    async def stats(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> dict[str, object] | None:
        del session
        return {
            "cpu_percent": 1.0,
            "memory_usage_bytes": 100,
            "memory_limit_bytes": 1000,
            "memory_percent": 10.0,
        }

    def container_name(self, *, session: KernelRuntimeSession) -> str | None:
        return self._session_key(session)

    def vscode_url(self, *, session: KernelRuntimeSession) -> str | None:
        return f"http://127.0.0.1/vscode/{self._session_key(session)}"

    def _session_key(self, session: KernelRuntimeSession) -> str:
        assert isinstance(session.value, str)
        return session.value


@pytest.fixture
def client(monkeypatch: pytest.MonkeyPatch, tmp_path: object) -> TestClient:
    agent_host_app = importlib.import_module("agent_host.app")
    host = AgentHost(runtime=StubRuntime())
    monkeypatch.setattr(agent_host_app, "host", host)
    skills_svc = SkillsService(skills_dir=str(tmp_path))
    monkeypatch.setattr(agent_host_app, "skills", skills_svc)
    from agent_host.gateways import GatewayHost  # noqa: PLC0415
    from test_gateways import FakeGatewayRuntime  # noqa: PLC0415

    gateways_host = GatewayHost(runtime=FakeGatewayRuntime())
    monkeypatch.setattr(agent_host_app, "gateways", gateways_host)
    return TestClient(agent_host_app.app)


def test_healthz(client: TestClient) -> None:
    response = client.get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"status": "ok"}


def test_info_returns_filtered_env(
    client: TestClient,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("AGENT_HOST_FOO", "foo-value")
    monkeypatch.setenv("AGENT_HOST_BAR", "bar-value")
    monkeypatch.setenv("UNRELATED_VAR", "should-not-appear")

    response = client.get("/info")

    assert response.status_code == 200
    payload = response.json()
    assert payload["service"] == "agent_host"
    assert payload["env_prefix"] == "AGENT_HOST_"
    assert payload["env"]["AGENT_HOST_FOO"] == "foo-value"
    assert payload["env"]["AGENT_HOST_BAR"] == "bar-value"
    assert "UNRELATED_VAR" not in payload["env"]


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


def test_message_stream_route(client: TestClient) -> None:
    created = client.post("/sessions", json={"harness": "copilot-cli"})
    session_id = created.json()["session_id"]

    with client.stream(
        "POST",
        f"/sessions/{session_id}/messages/stream",
        json={"message": "hello"},
    ) as response:
        lines = [json.loads(line) for line in response.iter_lines() if line]

    assert response.status_code == 200
    assert [line["type"] for line in lines] == [
        "session/start",
        "session/status",
        "text_delta",
        "session/status",
        "session/end",
    ]
    assert lines[2]["content"] == "hello"


def test_session_logs(client: TestClient) -> None:
    created = client.post("/sessions", json={"harness": "copilot-cli"})
    session_id = created.json()["session_id"]

    logs = client.get(f"/sessions/{session_id}/logs")

    assert logs.status_code == 200
    assert isinstance(logs.json()["lines"], list)


def test_session_logs_not_found(client: TestClient) -> None:
    response = client.get("/sessions/nonexistent/logs")

    assert response.status_code == 404


def test_skill_lifecycle(client: TestClient) -> None:
    created = client.post(
        "/skills",
        json={"skill_id": "my-skill", "files": {"SKILL.md": "# My Skill"}},
    )
    listed = client.get("/skills")
    fetched = client.get("/skills/my-skill")
    updated = client.put(
        "/skills/my-skill",
        json={"files": {"SKILL.md": "# Updated"}},
    )
    deleted = client.delete("/skills/my-skill")
    after_delete = client.get("/skills/my-skill")

    assert created.status_code == 200
    assert created.json()["skill_id"] == "my-skill"
    assert listed.status_code == 200
    assert len(listed.json()) == 1
    assert fetched.status_code == 200
    assert fetched.json()["files"]["SKILL.md"] == "# My Skill"
    assert updated.status_code == 200
    assert updated.json()["files"]["SKILL.md"] == "# Updated"
    assert deleted.status_code == 204
    assert after_delete.status_code == 404


def test_create_duplicate_skill_returns_409(client: TestClient) -> None:
    client.post(
        "/skills",
        json={"skill_id": "dup-skill", "files": {"SKILL.md": "# First"}},
    )
    response = client.post(
        "/skills",
        json={"skill_id": "dup-skill", "files": {"SKILL.md": "# Second"}},
    )

    assert response.status_code == 409


def test_invalid_skill_id_returns_422(client: TestClient) -> None:
    response = client.post(
        "/skills",
        json={"skill_id": "Bad Skill", "files": {"SKILL.md": "# Bad"}},
    )

    assert response.status_code == 422


def test_gateway_lifecycle(client: TestClient) -> None:
    created = client.post(
        "/gateways",
        json={
            "gateway_id": "echo-one",
            "gateway_type": "echo",
            "agent_id": "agent-x",
            "env": {"FOO": "bar"},
        },
    )
    listed = client.get("/gateways")
    fetched = client.get("/gateways/echo-one")
    logs = client.get("/gateways/echo-one/logs")
    deleted = client.delete("/gateways/echo-one")
    after = client.get("/gateways/echo-one")

    assert created.status_code == 200
    assert created.json()["gateway_id"] == "echo-one"
    assert created.json()["status"] == "running"
    assert listed.status_code == 200
    assert [g["gateway_id"] for g in listed.json()] == ["echo-one"]
    assert fetched.status_code == 200
    assert logs.status_code == 200
    assert logs.json()["lines"] == ["line-1", "line-2"]
    assert deleted.status_code == 204
    assert after.status_code == 404


def test_duplicate_gateway_returns_409(client: TestClient) -> None:
    payload: dict[str, object] = {
        "gateway_id": "dup-gw",
        "gateway_type": "echo",
        "agent_id": "agent",
        "env": dict[str, str](),
    }
    client.post("/gateways", json=payload)
    response = client.post("/gateways", json=payload)

    assert response.status_code == 409


def test_invalid_gateway_id_returns_422(client: TestClient) -> None:
    response = client.post(
        "/gateways",
        json={
            "gateway_id": "Bad Gateway",
            "gateway_type": "echo",
            "agent_id": "agent",
            "env": {},
        },
    )

    assert response.status_code == 422
