from __future__ import annotations

import importlib
import json
from dataclasses import asdict
from typing import TYPE_CHECKING, Any

import httpx
import pytest
from client_service.service import GatewayNotFoundError as _GatewayNotFound
from client_service.service import KernelNotFoundError
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


class StubClientService:
    def __init__(self) -> None:
        self.agents: dict[str, dict[str, object]] = {}
        self.sessions: dict[str, dict[str, Any]] = {}
        self.killed_kernels: list[str] = []
        self.skills: dict[str, dict[str, object]] = {}
        self.kernel_configs: dict[str, dict[str, object]] = {}
        self.gateways: dict[str, dict[str, object]] = {}
        self.autostart_called = False

    async def create_agent(
        self,
        *,
        agent_id: str,
        name: str,
        harness: HarnessName,
        system_prompt: str = "",
        skills: list[str] | None = None,
        env_vars: str = "",
    ) -> dict[str, object]:
        agent: dict[str, object] = {
            "agent_id": agent_id,
            "name": name,
            "harness": harness.value,
            "system_prompt": system_prompt,
            "skills": skills or [],
            "env_vars": env_vars,
            "created_at": "now",
            "updated_at": "now",
        }
        self.agents[str(agent["agent_id"])] = agent
        return agent

    async def list_harnesses(self) -> list[str]:
        return ["claude-code", "echo", "copilot-cli", "codex"]

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
        env_vars: str | None,
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
        if env_vars is not None:
            agent["env_vars"] = env_vars
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
        return {
            "session": self.sessions[session_id],
            "assistant_message": {
                "message_id": "msg-2",
                "session_id": session_id,
                "role": "assistant",
                "content": "hello",
                "created_at": "now",
            },
            "events": [asdict(event) for event in self._events()],
        }

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[dict[str, object]]:
        del message

        async def iterator() -> AsyncIterator[dict[str, object]]:
            for event in self._events():
                yield {"type": "event", "event": asdict(event)}
            yield {
                "type": "final",
                "session": self.sessions[session_id],
                "assistant_message": {
                    "message_id": "msg-2",
                    "session_id": session_id,
                    "role": "assistant",
                    "content": "hello",
                    "created_at": "now",
                },
                "events": [asdict(event) for event in self._events()],
            }

        return iterator()

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

    async def list_kernel_configs(self) -> list[dict[str, object]]:
        return list(self.kernel_configs.values())

    async def get_kernel_config(self, harness: HarnessName) -> dict[str, object]:
        existing = self.kernel_configs.get(harness.value)
        if existing is not None:
            return dict(existing)
        return {
            "harness": harness.value,
            "env_vars": "",
            "updated_at": None,
        }

    async def update_kernel_config(
        self,
        harness: HarnessName,
        env_vars: str,
    ) -> dict[str, object]:
        record: dict[str, object] = {
            "harness": harness.value,
            "env_vars": env_vars,
            "updated_at": "now",
        }
        self.kernel_configs[harness.value] = record
        return dict(record)

    async def info(self) -> dict[str, object]:
        return {
            "client_service": {
                "service": "client_service",
                "env_prefix": "CLIENT_SERVICE_",
                "env": {"CLIENT_SERVICE_STUB": "1"},
            },
            "agent_host": {
                "service": "agent_host",
                "env_prefix": "AGENT_HOST_",
                "env": {"AGENT_HOST_STUB": "1"},
            },
        }

    async def list_gateways(
        self,
        *,
        include_secrets: bool = False,
    ) -> list[dict[str, object]]:
        del include_secrets
        return list(self.gateways.values())

    async def get_gateway(
        self,
        gateway_id: str,
        *,
        include_secrets: bool = False,
    ) -> dict[str, object]:
        del include_secrets
        if gateway_id not in self.gateways:
            raise _GatewayNotFound(gateway_id)
        return dict(self.gateways[gateway_id])

    async def create_gateway(
        self,
        *,
        gateway_id: str,
        name: str,
        gateway_type: object,
        agent_id: str,
        enabled: bool = False,
        env_vars: str = "",
        secrets: dict[str, str] | None = None,
    ) -> dict[str, object]:
        del env_vars, secrets
        record: dict[str, object] = {
            "gateway_id": gateway_id,
            "name": name,
            "gateway_type": str(getattr(gateway_type, "value", gateway_type)),
            "agent_id": agent_id,
            "enabled": enabled,
            "status": "running" if enabled else "stopped",
            "secret_keys": [],
        }
        self.gateways[gateway_id] = record
        return dict(record)

    async def update_gateway(
        self,
        gateway_id: str,
        *,
        name: str | None = None,
        agent_id: str | None = None,
        enabled: bool | None = None,
        env_vars: str | None = None,
        secrets: dict[str, str] | None = None,
    ) -> dict[str, object]:
        del env_vars, secrets
        record = dict(self.gateways[gateway_id])
        if name is not None:
            record["name"] = name
        if agent_id is not None:
            record["agent_id"] = agent_id
        if enabled is not None:
            record["enabled"] = enabled
            record["status"] = "running" if enabled else "stopped"
        self.gateways[gateway_id] = record
        return dict(record)

    async def delete_gateway(self, gateway_id: str) -> None:
        if gateway_id not in self.gateways:
            raise _GatewayNotFound(gateway_id)
        del self.gateways[gateway_id]

    async def start_gateway(self, gateway_id: str) -> dict[str, object]:
        record = dict(self.gateways[gateway_id])
        record["status"] = "running"
        self.gateways[gateway_id] = record
        return dict(record)

    async def stop_gateway(self, gateway_id: str) -> dict[str, object]:
        record = dict(self.gateways[gateway_id])
        record["status"] = "stopped"
        self.gateways[gateway_id] = record
        return dict(record)

    async def gateway_logs(self, gateway_id: str) -> list[str]:
        if gateway_id not in self.gateways:
            raise _GatewayNotFound(gateway_id)
        return ["line1", "line2"]

    async def autostart_enabled_gateways(self) -> None:
        self.autostart_called = True

    def _events(self) -> list[KernelEvent]:
        return [
            session_start("host-1", "copilot-cli"),
            status_event(KernelStatus.BUSY),
            text_delta("hello"),
            session_end(),
        ]


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


def test_create_agent_accepts_explicit_harness(client: TestClient) -> None:
    response = client.post(
        "/agents",
        json={
            "agent_id": "agent-codex",
            "name": "Agent Codex",
            "harness": "codex",
        },
    )

    assert response.status_code == 200
    assert response.json()["harness"] == "codex"


def test_list_harnesses_route(client: TestClient) -> None:
    response = client.get("/harnesses")

    assert response.status_code == 200
    assert response.json() == ["claude-code", "echo", "copilot-cli", "codex"]


def test_info_route_aggregates_sections(client: TestClient) -> None:
    response = client.get("/info")

    assert response.status_code == 200
    payload = response.json()
    assert payload["client_service"]["env"]["CLIENT_SERVICE_STUB"] == "1"
    assert payload["agent_host"]["env"]["AGENT_HOST_STUB"] == "1"
    assert "webui" not in payload


def test_message_stream_route(client: TestClient) -> None:
    client.post("/agents", json={"agent_id": "agent-one", "name": "Agent One"})
    created_session = client.post(
        "/sessions",
        json={
            "agent_id": "agent-one",
            "channel_name": "webui",
            "client_type": "webui",
        },
    )

    with client.stream(
        "POST",
        f"/sessions/{created_session.json()['session_id']}/messages/stream",
        json={"message": "hello"},
    ) as response:
        chunks = [json.loads(line) for line in response.iter_lines() if line]

    assert response.status_code == 200
    assert [chunk["type"] for chunk in chunks] == [
        "event",
        "event",
        "event",
        "event",
        "final",
    ]
    assert chunks[-1]["assistant_message"]["content"] == "hello"


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


def test_kernel_config_get_returns_empty_default(client: TestClient) -> None:
    response = client.get("/kernel-configs/opencode")

    assert response.status_code == 200
    assert response.json() == {
        "harness": "opencode",
        "env_vars": "",
        "updated_at": None,
    }


def test_kernel_config_put_and_get_roundtrip(client: TestClient) -> None:
    put_response = client.put(
        "/kernel-configs/opencode",
        json={"env_vars": "OPENCODE_MODEL=gpt-5\nOPENCODE_AGENT=plan"},
    )
    get_response = client.get("/kernel-configs/opencode")
    list_response = client.get("/kernel-configs")

    assert put_response.status_code == 200
    assert put_response.json()["env_vars"] == (
        "OPENCODE_MODEL=gpt-5\nOPENCODE_AGENT=plan"
    )
    assert get_response.status_code == 200
    assert get_response.json()["env_vars"] == (
        "OPENCODE_MODEL=gpt-5\nOPENCODE_AGENT=plan"
    )
    assert list_response.status_code == 200
    assert len(list_response.json()) == 1


def test_kernel_config_invalid_harness_returns_422(client: TestClient) -> None:
    response = client.get("/kernel-configs/not-a-harness")

    assert response.status_code == 422


def test_list_gateway_types(client: TestClient) -> None:
    response = client.get("/gateway-types")
    assert response.status_code == 200
    assert "echo" in response.json()


def test_gateway_type_schema_echo(client: TestClient) -> None:
    response = client.get("/gateway-types/echo/schema")
    assert response.status_code == 200
    body = response.json()
    assert body == {"fields": []}


def test_gateway_type_schema_discord(client: TestClient) -> None:
    response = client.get("/gateway-types/discord/schema")
    assert response.status_code == 200
    body = response.json()
    keys = [f["key"] for f in body["fields"]]
    assert "DISCORD_BOT_TOKEN" in keys
    assert "DISCORD_OWNER_USER_ID" in keys
    token_field = next(f for f in body["fields"] if f["key"] == "DISCORD_BOT_TOKEN")
    assert token_field["kind"] == "secret"
    assert token_field["required"] is True


def test_gateway_type_schema_unknown_returns_422(client: TestClient) -> None:
    response = client.get("/gateway-types/not-a-type/schema")
    assert response.status_code == 422


def test_gateway_routes_lifecycle(client: TestClient) -> None:
    client.post("/agents", json={"agent_id": "agent-one", "name": "Agent One"})

    created = client.post(
        "/gateways",
        json={
            "gateway_id": "echo-bridge",
            "name": "Echo Bridge",
            "gateway_type": "echo",
            "agent_id": "agent-one",
            "enabled": True,
        },
    )
    listed = client.get("/gateways")
    fetched = client.get("/gateways/echo-bridge")
    logs = client.get("/gateways/echo-bridge/logs")
    stopped = client.post("/gateways/echo-bridge/stop")
    started = client.post("/gateways/echo-bridge/start")
    deleted = client.delete("/gateways/echo-bridge")

    assert created.status_code == 200
    assert created.json()["status"] == "running"
    assert listed.status_code == 200
    assert len(listed.json()) == 1
    assert fetched.status_code == 200
    assert logs.status_code == 200
    assert logs.json() == {"lines": ["line1", "line2"]}
    assert stopped.json()["status"] == "stopped"
    assert started.json()["status"] == "running"
    assert deleted.status_code == 204


def test_gateway_invalid_id_returns_422(client: TestClient) -> None:
    response = client.post(
        "/gateways",
        json={
            "gateway_id": "Bad Id",
            "name": "Bad",
            "gateway_type": "echo",
            "agent_id": "agent-one",
        },
    )
    assert response.status_code == 422


def test_gateway_unknown_returns_404(client: TestClient) -> None:
    assert client.get("/gateways/missing").status_code == 404
    assert client.delete("/gateways/missing").status_code == 404
    assert client.get("/gateways/missing/logs").status_code == 404
