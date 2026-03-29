from __future__ import annotations

import importlib
from typing import Any

import pytest
from fastapi.testclient import TestClient


class StubClientServiceClient:
    async def list_agents(self) -> list[dict[str, Any]]:
        return [
            {
                "agent_id": "agent-1",
                "name": "Agent One",
                "harness": "copilot-cli",
            },
        ]

    async def create_agent(
        self,
        *,
        agent_id: str,
        name: str,
        system_prompt: str,
    ) -> dict[str, Any]:
        del agent_id
        del system_prompt
        return {"agent_id": "agent-1", "name": name, "harness": "copilot-cli"}

    async def list_sessions(self) -> list[dict[str, Any]]:
        return [
            {
                "session_id": "session-1",
                "status": "idle",
                "message_count": 1,
            },
        ]

    async def create_session(self, *, agent_id: str, cwd: str | None) -> dict[str, Any]:
        del agent_id, cwd
        return {"session_id": "session-1"}

    async def get_session(self, session_id: str) -> dict[str, Any]:
        return {
            "session_id": session_id,
            "agent_id": "agent-1",
            "status": "done",
            "messages": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "world"},
            ],
        }

    async def send_message(self, session_id: str, message: str) -> dict[str, Any]:
        del session_id, message
        return {}

    async def reset_session(self, session_id: str) -> dict[str, Any]:
        del session_id
        return {}


@pytest.fixture
def client(monkeypatch: pytest.MonkeyPatch) -> TestClient:
    webui_app = importlib.import_module("webui.app")
    monkeypatch.setattr(webui_app, "client_service", StubClientServiceClient())
    return TestClient(webui_app.app)


def test_index_page_renders(client: TestClient) -> None:
    response = client.get("/")

    assert response.status_code == 200
    assert "Create Agent" in response.text
    assert "Agent One" in response.text


def test_session_page_and_forms(client: TestClient) -> None:
    created = client.post(
        "/agents",
        data={
            "agent_id": "agent-two",
            "name": "Agent Two",
            "system_prompt": "Be precise.",
        },
        follow_redirects=False,
    )
    started = client.post(
        "/sessions",
        data={"agent_id": "agent-1", "cwd": "C:/work"},
        follow_redirects=False,
    )
    session = client.get("/sessions/session-1")

    assert created.status_code == 303
    assert started.status_code == 303
    assert session.status_code == 200
    assert "world" in session.text
