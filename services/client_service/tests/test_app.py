from __future__ import annotations

import importlib
from dataclasses import asdict
from typing import TYPE_CHECKING, Any

import pytest
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
        self.agents: dict[str, dict[str, str]] = {}
        self.sessions: dict[str, dict[str, Any]] = {}
        self.channels: dict[str, dict[str, Any]] = {}

    async def create_agent(
        self,
        *,
        agent_id: str,
        name: str,
        harness: HarnessName,
        system_prompt: str = "",
    ) -> dict[str, str]:
        agent = {
            "agent_id": agent_id,
            "name": name,
            "harness": harness.value,
            "system_prompt": system_prompt,
            "created_at": "now",
            "updated_at": "now",
        }
        self.agents[agent["agent_id"]] = agent
        return agent

    async def list_agents(self) -> list[dict[str, str]]:
        return list(self.agents.values())

    async def get_agent(self, agent_id: str) -> dict[str, str]:
        return self.agents[agent_id]

    async def update_agent(
        self,
        agent_id: str,
        *,
        name: str | None,
        harness: HarnessName | None,
        system_prompt: str | None,
    ) -> dict[str, str]:
        agent = self.agents[agent_id]
        if name is not None:
            agent["name"] = name
        if harness is not None:
            agent["harness"] = harness.value
        if system_prompt is not None:
            agent["system_prompt"] = system_prompt
        return agent

    async def delete_agent(self, agent_id: str) -> None:
        del self.agents[agent_id]

    async def create_session(
        self,
        *,
        agent_id: str,
        cwd: str | None,
    ) -> dict[str, object]:
        session: dict[str, object] = {
            "session_id": "session-1",
            "agent_id": agent_id,
            "agent_host_session_id": "host-1",
            "status": "idle",
            "cwd": cwd,
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

    async def register_channel(
        self,
        *,
        agent_id: str,
        name: str,
        channel_type: str,
        cwd: str | None,
    ) -> dict[str, str | None]:
        self.sessions["session-1"] = {
            "session_id": "session-1",
            "agent_id": agent_id,
            "agent_host_session_id": "host-1",
            "status": "idle",
            "cwd": cwd,
            "created_at": "now",
            "updated_at": "now",
            "message_count": 0,
        }
        channel = {
            "channel_id": "channel-1",
            "channel_type": channel_type,
            "agent_id": agent_id,
            "session_id": "session-1",
            "name": name,
            "cwd": cwd,
            "created_at": "now",
            "updated_at": "now",
        }
        self.channels[str(channel["channel_id"])] = channel
        return channel

    async def list_channels(self) -> list[dict[str, str | None]]:
        return list(self.channels.values())

    async def get_channel(self, channel_id: str) -> dict[str, str | None]:
        return self.channels[channel_id]

    async def list_channel_messages(self, channel_id: str) -> list[dict[str, str]]:
        del channel_id
        return await self.list_messages("session-1")

    async def send_channel_message(
        self,
        channel_id: str,
        message: str,
    ) -> dict[str, object]:
        del channel_id
        return await self.send_message("session-1", message)

    async def reset_channel(self, channel_id: str) -> dict[str, str | None]:
        return self.channels[channel_id]

    async def delete_channel(self, channel_id: str) -> None:
        del self.channels[channel_id]


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
        json={"agent_id": str(created_agent.json()["agent_id"]), "cwd": "C:/work"},
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
    assert sent.json()["assistant_message"]["content"] == "hello"
    assert listed_messages.json()["messages"][0]["content"] == "hello"


def test_channel_routes(client: TestClient) -> None:
    created_agent = client.post(
        "/agents",
        json={"agent_id": "agent-one", "name": "Agent One"},
    )
    registered = client.post(
        "/channels",
        json={
            "agent_id": str(created_agent.json()["agent_id"]),
            "name": "terminal-1",
            "channel_type": "cli",
            "cwd": "C:/work",
        },
    )
    listed = client.get("/channels")
    sent = client.post(
        f"/channels/{registered.json()['channel_id']}/messages",
        json={"message": "hello"},
    )
    messages = client.get(f"/channels/{registered.json()['channel_id']}/messages")

    assert registered.status_code == 200
    assert listed.status_code == 200
    assert sent.status_code == 200
    assert messages.status_code == 200
    assert listed.json()[0]["name"] == "terminal-1"
    assert sent.json()["assistant_message"]["content"] == "hello"


def test_invalid_agent_id_rejected(client: TestClient) -> None:
    response = client.post(
        "/agents",
        json={"agent_id": "Bad Agent", "name": "Agent One"},
    )

    assert response.status_code == 422
