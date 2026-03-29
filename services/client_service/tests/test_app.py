from __future__ import annotations

import importlib
from dataclasses import asdict
from typing import TYPE_CHECKING, Any

import httpx
import pytest
from client_service.service import (
    KernelNotFoundError,
)
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


class StubClientService:
    def __init__(self) -> None:
        self.agents: dict[str, dict[str, object]] = {}
        self.sessions: dict[str, dict[str, Any]] = {}
        self.killed_kernels: list[str] = []
        self.skills: dict[str, dict[str, object]] = {}

    async def create_agent(
        self,
        *,
        agent_id: str,
        name: str,
        harness: HarnessName,
        system_prompt: str = "",
        skills: list[str] | None = None,
    ) -> dict[str, object]:
        agent: dict[str, object] = {
            "agent_id": agent_id,
            "name": name,
            "harness": harness.value,
            "system_prompt": system_prompt,
            "skills": skills or [],
            "created_at": "now",
            "updated_at": "now",
        }
        self.agents[str(agent["agent_id"])] = agent
        return agent

    async def list_agents(self) -> list[dict[str, object]]:
        return list(self.agents.values())

    async def get_agent(self, agent_id: str) -> dict[str, object]:
        return self.agents[agent_id]

    async def update_agent(
        self,
        agent_id: str,
        *,
        name: str | None,
        harness: HarnessName | None,
        system_prompt: str | None,
        skills: list[str] | None,
    ) -> dict[str, object]:
        agent = self.agents[agent_id]
        if name is not None:
            agent["name"] = name
        if harness is not None:
            agent["harness"] = harness.value
        if system_prompt is not None:
            agent["system_prompt"] = system_prompt
        if skills is not None:
            agent["skills"] = list(skills)
        return agent

    async def delete_agent(self, agent_id: str) -> None:
        del self.agents[agent_id]

    async def create_session(
        self,
        *,
        agent_id: str,
        channel_name: str | None = None,
        client_type: str | None = None,
    ) -> dict[str, object]:
        session: dict[str, object] = {
            "session_id": "session-1",
            "agent_id": agent_id,
            "agent_host_session_id": "host-1",
            "status": "idle",
            "channel_name": channel_name,
            "client_type": client_type,
            "created_at": "now",
            "updated_at": "now",
            "message_count": 0,
        }
        self.sessions[str(session["session_id"])] = session
        return session

    async def list_sessions(self) -> list[dict[str, object]]:
        return list(self.sessions.values())

    async def get_session(self, session_id: str) -> dict[str, object]:
        session = dict(self.sessions[session_id])
        session["messages"] = [
            {
                "message_id": "msg-1",
                "session_id": session_id,
                "role": "assistant",
                "content": "hello",
                "created_at": "now",
            },
        ]
        return session

    async def list_messages(self, session_id: str) -> list[dict[str, str]]:
        del session_id
        return [
            {
                "message_id": "msg-1",
                "session_id": "session-1",
                "role": "assistant",
                "content": "hello",
                "created_at": "now",
            },
        ]

    async def send_message(self, session_id: str, message: str) -> dict[str, object]:
        del message
        events: list[KernelEvent] = [
            session_start("host-1", "copilot-cli"),
            status_event(KernelStatus.BUSY),
            text_delta("hello"),
            session_end(),
        ]
        return {
            "session": self.sessions[session_id],
            "assistant_message": {
                "message_id": "msg-2",
                "session_id": session_id,
                "role": "assistant",
                "content": "hello",
                "created_at": "now",
            },
            "events": [asdict(event) for event in events],
        }

    async def reset_session(self, session_id: str) -> dict[str, object]:
        return self.sessions[session_id]

    async def delete_session(self, session_id: str) -> None:
        del self.sessions[session_id]

    async def list_kernels(self) -> list[dict[str, object]]:
        return [
            {
                "session_id": "host-1",
                "harness": "copilot-cli",
                "status": "idle",
                "turns": 1,
                "resume_token": "resume-1",
                "additional_paths": [],
                "client_session_ids": ["session-1"],
                "channel_names": ["webui"],
                "agent_ids": ["agent-one"],
            },
        ]

    async def kill_kernel(self, kernel_session_id: str) -> None:
        if kernel_session_id != "host-1":
            raise KernelNotFoundError(kernel_session_id)
        self.killed_kernels.append(kernel_session_id)

    async def kernel_logs(self, kernel_session_id: str) -> list[str]:
        if kernel_session_id != "host-1":
            raise KernelNotFoundError(kernel_session_id)
        return ['{"type":"stub","data":{}}']

    async def create_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> dict[str, object]:
        skill: dict[str, object] = {"skill_id": skill_id, "files": files}
        self.skills[skill_id] = skill
        return skill

    async def get_skill(self, skill_id: str) -> dict[str, object]:
        if skill_id not in self.skills:
            _raise_skill_not_found("GET", skill_id)
        return dict(self.skills[skill_id])

    async def list_skills(self) -> list[dict[str, object]]:
        return [{"skill_id": sid} for sid in self.skills]

    async def update_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> dict[str, object]:
        if skill_id not in self.skills:
            _raise_skill_not_found("PUT", skill_id)
        self.skills[skill_id] = {"skill_id": skill_id, "files": files}
        return dict(self.skills[skill_id])

    async def delete_skill(self, skill_id: str) -> None:
        if skill_id not in self.skills:
            _raise_skill_not_found("DELETE", skill_id)
        del self.skills[skill_id]


def _raise_skill_not_found(method: str, skill_id: str) -> None:
    msg = f"skill not found: {skill_id}"
    raise httpx.HTTPStatusError(
        msg,
        request=httpx.Request(method, f"/skills/{skill_id}"),
        response=httpx.Response(404),
    )


@pytest.fixture
def client(monkeypatch: pytest.MonkeyPatch) -> TestClient:
    client_service_app = importlib.import_module("client_service.app")
    monkeypatch.setattr(client_service_app, "service", StubClientService())
    return TestClient(client_service_app.app)


def test_agent_and_session_routes(client: TestClient) -> None:
    created_agent = client.post(
        "/agents",
        json={"agent_id": "agent-one", "name": "Agent One"},
    )
    created_session = client.post(
        "/sessions",
        json={
            "agent_id": str(created_agent.json()["agent_id"]),
            "channel_name": "webui",
            "client_type": "webui",
        },
    )
    sent = client.post(
        f"/sessions/{created_session.json()['session_id']}/messages",
        json={"message": "hello"},
    )
    session_id = created_session.json()["session_id"]
    listed_messages = client.get(f"/sessions/{session_id}/messages")

    assert created_agent.status_code == 200
    assert created_session.status_code == 200
    assert sent.status_code == 200
    assert listed_messages.status_code == 200
    assert created_session.json()["channel_name"] == "webui"
    assert sent.json()["assistant_message"]["content"] == "hello"
    assert listed_messages.json()["messages"][0]["content"] == "hello"


def test_kernel_routes(client: TestClient) -> None:
    response = client.get("/kernels")

    assert response.status_code == 200
    assert response.json()[0]["channel_names"] == ["webui"]


def test_kernel_logs(client: TestClient) -> None:
    response = client.get("/kernels/host-1/logs")

    assert response.status_code == 200
    assert isinstance(response.json()["lines"], list)


def test_kernel_logs_not_found(client: TestClient) -> None:
    response = client.get("/kernels/nonexistent/logs")

    assert response.status_code == 404


def test_invalid_agent_id_rejected(client: TestClient) -> None:
    response = client.post(
        "/agents",
        json={"agent_id": "Bad Agent", "name": "Agent One"},
    )

    assert response.status_code == 422


def test_kill_kernel_returns_204(client: TestClient) -> None:
    response = client.delete("/kernels/host-1")

    assert response.status_code == 204


def test_kill_kernel_not_found_returns_404(client: TestClient) -> None:
    response = client.delete("/kernels/nonexistent")

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


def test_invalid_skill_id_returns_422(client: TestClient) -> None:
    response = client.post(
        "/skills",
        json={"skill_id": "Bad Skill", "files": {"SKILL.md": "# Bad"}},
    )

    assert response.status_code == 422
