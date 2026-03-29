from __future__ import annotations

from typing import TYPE_CHECKING, Any, cast

import pytest
from client_service.service import (
    AgentAlreadyExistsError,
    AgentNotFoundError,
    ChannelNotFoundError,
    ClientService,
    InvalidAgentIdError,
    SessionNotFoundError,
)
from kernel.events import (
    KernelEvent,
    KernelStatus,
    session_end,
    session_start,
    status_event,
    text_delta,
)

if TYPE_CHECKING:
    from client_service.agent_host_client import AgentHostClient
    from kernel_host.registry import HarnessName


class StubAgentHostClient:
    def __init__(self) -> None:
        self.created: list[dict[str, Any]] = []
        self.sent: list[tuple[str, str]] = []
        self.destroyed: list[str] = []
        self._sessions: dict[str, dict[str, str]] = {}

    async def create_session(
        self,
        *,
        harness: HarnessName,
        cwd: str | None,
    ) -> dict[str, object]:
        session_id = f"host-{len(self.created) + 1}"
        self.created.append({"harness": harness, "cwd": cwd, "session_id": session_id})
        self._sessions[session_id] = {"session_id": session_id, "status": "idle"}
        session: dict[str, object] = {"session_id": session_id, "status": "idle"}
        return session

    async def get_session(self, session_id: str) -> dict[str, object]:
        return {
            "session_id": self._sessions[session_id]["session_id"],
            "status": self._sessions[session_id]["status"],
        }

    async def list_sessions(self) -> list[dict[str, object]]:
        return [await self.get_session(session_id) for session_id in self._sessions]

    async def send_message(self, session_id: str, message: str) -> list[KernelEvent]:
        self.sent.append((session_id, message))
        self._sessions[session_id]["status"] = "done"
        return [
            session_start(session_id, "copilot-cli"),
            status_event(KernelStatus.BUSY),
            text_delta("hello"),
            text_delta(" world"),
            status_event(KernelStatus.DONE),
            session_end(),
        ]

    async def history(self, session_id: str) -> list[list[KernelEvent]]:
        del session_id
        return []

    async def reset_session(self, session_id: str) -> dict[str, object]:
        new_session_id = f"{session_id}-reset"
        self._sessions[new_session_id] = {
            "session_id": new_session_id,
            "status": "idle",
        }
        return {
            "session_id": new_session_id,
            "status": "idle",
        }

    async def destroy_session(self, session_id: str) -> None:
        self.destroyed.append(session_id)


@pytest.mark.asyncio
async def test_agent_and_session_lifecycle() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    agent = await service.create_agent(agent_id="test-agent", name="Test Agent")
    session = await service.create_session(agent_id=agent["agent_id"], cwd="C:/work")
    session_id = str(session["session_id"])
    reply = await service.send_message(session_id, "hello")
    messages = await service.list_messages(session_id)
    reset = await service.reset_session(session_id)
    assistant_message = cast("dict[str, object]", reply["assistant_message"])

    assert agent["harness"] == "copilot-cli"
    assert session["agent_id"] == agent["agent_id"]
    assert assistant_message["content"] == "hello world"
    assert len(messages) == 2
    assert str(reset["agent_host_session_id"]).endswith("-reset")
    assert await service.list_messages(session_id) == []


@pytest.mark.asyncio
async def test_delete_agent_cascades_sessions() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    agent = await service.create_agent(agent_id="test-agent", name="Test Agent")
    agent_id = agent["agent_id"]
    session = await service.create_session(agent_id=agent["agent_id"], cwd=None)
    session_id = str(session["session_id"])
    upstream_session_id = str(session["agent_host_session_id"])

    await service.delete_agent(agent_id)

    assert upstream.destroyed == [upstream_session_id]
    with pytest.raises(AgentNotFoundError):
        await service.get_agent(agent_id)
    with pytest.raises(SessionNotFoundError):
        await service.get_session(session_id)


@pytest.mark.asyncio
async def test_channel_registration_and_reset_reuse_session() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    agent = await service.create_agent(agent_id="channel-agent", name="Channel Agent")
    channel = await service.register_channel(
        agent_id=agent["agent_id"],
        name="terminal-1",
        cwd="C:/work",
    )
    channel_id = str(channel["channel_id"])
    session_id = str(channel["session_id"])
    reply = await service.send_channel_message(channel_id, "hello")
    messages = await service.list_channel_messages(channel_id)
    reset = await service.reset_channel(channel_id)
    assistant_message = cast("dict[str, object]", reply["assistant_message"])

    assert channel["name"] == "terminal-1"
    assert channel["session_id"] == session_id
    assert assistant_message["content"] == "hello world"
    assert len(messages) == 2
    assert reset["session_id"] == session_id
    assert await service.list_channel_messages(channel_id) == []


@pytest.mark.asyncio
async def test_missing_records_raise() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    with pytest.raises(AgentNotFoundError):
        await service.create_session(agent_id="missing", cwd=None)

    with pytest.raises(SessionNotFoundError):
        await service.send_message("missing", "hello")

    with pytest.raises(ChannelNotFoundError):
        await service.send_channel_message("missing", "hello")


@pytest.mark.asyncio
async def test_agent_id_validation_and_uniqueness() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    with pytest.raises(InvalidAgentIdError):
        await service.create_agent(agent_id="Bad Agent", name="Bad Agent")

    await service.create_agent(agent_id="valid-agent", name="Valid Agent")

    with pytest.raises(AgentAlreadyExistsError):
        await service.create_agent(agent_id="valid-agent", name="Duplicate")
