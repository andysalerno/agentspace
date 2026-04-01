from __future__ import annotations

from typing import TYPE_CHECKING, Any, cast

import pytest
from client_service.models import ClientType
from client_service.service import (
    AgentAlreadyExistsError,
    AgentNotFoundError,
    ClientService,
    InvalidAgentIdError,
    KernelNotFoundError,
    SessionNotFoundError,
)
from kernel.events import (
    KernelEvent,
    KernelStatus,
    session_end,
    session_start,
    status_event,
    text_delta,
    tool_call,
    tool_result,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from client_service.agent_host_client import AgentHostClient
    from kernel_host.registry import HarnessName


class StubAgentHostClient:
    def __init__(self) -> None:
        self.created: list[dict[str, Any]] = []
        self.sent: list[tuple[str, str]] = []
        self.destroyed: list[str] = []
        self._sessions: dict[str, dict[str, str]] = {}
        self._skills: dict[str, dict[str, object]] = {}

    async def create_session(
        self,
        *,
        harness: HarnessName,
        skills: list[str] | None = None,
    ) -> dict[str, object]:
        del skills
        session_id = f"host-{len(self.created) + 1}"
        self.created.append({"harness": harness, "session_id": session_id})
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
        return [event async for event in self.stream_message(session_id, message)]

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[KernelEvent]:
        self.sent.append((session_id, message))
        events = [
            session_start(session_id, "copilot-cli"),
            status_event(KernelStatus.BUSY),
            text_delta("hello"),
            text_delta(" world"),
            status_event(KernelStatus.DONE),
            session_end(),
        ]

        async def iterator() -> AsyncIterator[KernelEvent]:
            for event in events:
                yield event
            self._sessions[session_id]["status"] = "done"

        return iterator()

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
        self._sessions.pop(session_id, None)

    async def logs(self, session_id: str) -> list[str]:
        if session_id not in self._sessions:
            msg = f"session not found: {session_id}"
            raise KeyError(msg)
        return ['{"type":"stub","data":{}}']

    async def create_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> dict[str, object]:
        skill: dict[str, object] = {"skill_id": skill_id, "files": files}
        self._skills[skill_id] = skill
        return skill

    async def get_skill(self, skill_id: str) -> dict[str, object]:
        if skill_id not in self._skills:
            msg = f"skill not found: {skill_id}"
            raise KeyError(msg)
        return dict(self._skills[skill_id])

    async def list_skills(self) -> list[dict[str, object]]:
        return [{"skill_id": sid} for sid in self._skills]

    async def update_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> dict[str, object]:
        if skill_id not in self._skills:
            msg = f"skill not found: {skill_id}"
            raise KeyError(msg)
        self._skills[skill_id] = {"skill_id": skill_id, "files": files}
        return dict(self._skills[skill_id])

    async def delete_skill(self, skill_id: str) -> None:
        if skill_id not in self._skills:
            msg = f"skill not found: {skill_id}"
            raise KeyError(msg)
        del self._skills[skill_id]


@pytest.mark.asyncio
async def test_agent_and_session_lifecycle() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    agent = await service.create_agent(agent_id="test-agent", name="Test Agent")
    session = await service.create_session(
        agent_id=str(agent["agent_id"]),
        channel_name="webui",
        client_type=ClientType.WEBUI,
    )
    session_id = str(session["session_id"])
    reply = await service.send_message(session_id, "hello")
    messages = await service.list_messages(session_id)
    reset = await service.reset_session(session_id)
    assistant_message = cast("dict[str, object]", reply["assistant_message"])

    assert agent["harness"] == "copilot-cli"
    assert session["agent_id"] == agent["agent_id"]
    assert session["channel_name"] == "webui"
    assert session["client_type"] == "webui"
    assert assistant_message["content"] == "hello world"
    assert "type" not in reply
    assert len(messages) == 2
    assert str(reset["agent_host_session_id"]).endswith("-reset")
    assert await service.list_messages(session_id) == []


@pytest.mark.asyncio
async def test_list_harnesses_returns_registered_harnesses() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    assert await service.list_harnesses() == [
        "claude-code",
        "echo",
        "copilot-cli",
        "codex",
    ]


@pytest.mark.asyncio
async def test_tool_calls_extracted_into_assistant_message() -> None:
    """Tool call events should be extracted and stored with the assistant message."""

    class ToolCallStub(StubAgentHostClient):
        async def send_message(
            self,
            session_id: str,
            message: str,
        ) -> list[KernelEvent]:
            return [event async for event in self.stream_message(session_id, message)]

        def stream_message(
            self,
            session_id: str,
            message: str,
        ) -> AsyncIterator[KernelEvent]:
            self.sent.append((session_id, message))
            events = [
                session_start(session_id, "copilot-cli"),
                status_event(KernelStatus.BUSY),
                tool_call("read_file", {"path": "src/foo.py"}),
                tool_result("read_file", "print('hello')"),
                tool_call("write_file", {"path": "src/bar.py", "content": "x = 1"}),
                tool_result("write_file", "ok"),
                text_delta("Done editing."),
                status_event(KernelStatus.DONE),
                session_end(),
            ]

            async def iterator() -> AsyncIterator[KernelEvent]:
                for event in events:
                    yield event
                self._sessions[session_id]["status"] = "done"

            return iterator()

    upstream = ToolCallStub()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    agent = await service.create_agent(agent_id="tool-agent", name="Tool Agent")
    session = await service.create_session(agent_id=str(agent["agent_id"]))
    session_id = str(session["session_id"])

    reply = await service.send_message(session_id, "edit some files")
    assistant_message = cast("dict[str, object]", reply["assistant_message"])

    assert assistant_message["content"] == "Done editing."
    assert assistant_message["tool_calls"] == [
        {
            "tool": "read_file",
            "input": '{\n  "path": "src/foo.py"\n}',
            "output": "print('hello')",
        },
        {
            "tool": "write_file",
            "input": '{\n  "path": "src/bar.py",\n  "content": "x = 1"\n}',
            "output": "ok",
        },
    ]

    # Verify tool calls persist in session history
    messages = await service.list_messages(session_id)
    assert len(messages) == 2
    assert messages[1]["tool_calls"] == [
        {
            "tool": "read_file",
            "input": '{\n  "path": "src/foo.py"\n}',
            "output": "print('hello')",
        },
        {
            "tool": "write_file",
            "input": '{\n  "path": "src/bar.py",\n  "content": "x = 1"\n}',
            "output": "ok",
        },
    ]


@pytest.mark.asyncio
async def test_stream_message_yields_events_then_final_payload() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    agent = await service.create_agent(agent_id="stream-agent", name="Stream Agent")
    session = await service.create_session(agent_id=str(agent["agent_id"]))
    session_id = str(session["session_id"])

    chunks = [chunk async for chunk in service.stream_message(session_id, "hello")]
    messages = await service.list_messages(session_id)

    assert [str(chunk["type"]) for chunk in chunks] == [
        "event",
        "event",
        "event",
        "event",
        "event",
        "event",
        "final",
    ]
    assert chunks[-1]["assistant_message"] == messages[1]
    assert messages[0]["content"] == "hello"
    assert messages[1]["content"] == "hello world"


@pytest.mark.asyncio
async def test_delete_agent_cascades_sessions() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    agent = await service.create_agent(agent_id="test-agent", name="Test Agent")
    agent_id = str(agent["agent_id"])
    session = await service.create_session(agent_id=agent_id)
    session_id = str(session["session_id"])
    upstream_session_id = str(session["agent_host_session_id"])

    await service.delete_agent(agent_id)

    assert upstream.destroyed == [upstream_session_id]
    with pytest.raises(AgentNotFoundError):
        await service.get_agent(agent_id)
    with pytest.raises(SessionNotFoundError):
        await service.get_session(session_id)


@pytest.mark.asyncio
async def test_list_kernels_includes_client_session_and_channel_names() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    agent = await service.create_agent(agent_id="kernel-agent", name="Kernel Agent")
    session = await service.create_session(
        agent_id=str(agent["agent_id"]),
        channel_name="terminal-1",
        client_type=ClientType.CLI,
    )

    kernels = await service.list_kernels()

    assert len(kernels) == 1
    kernel = next(
        kernel
        for kernel in kernels
        if session["session_id"] in cast("list[str]", kernel["client_session_ids"])
    )
    assert kernel["agent_ids"] == ["kernel-agent"]
    assert kernel["channel_names"] == ["terminal-1"]


@pytest.mark.asyncio
async def test_missing_records_raise() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    with pytest.raises(AgentNotFoundError):
        await service.create_session(agent_id="missing")

    with pytest.raises(SessionNotFoundError):
        await service.send_message("missing", "hello")


@pytest.mark.asyncio
async def test_agent_id_validation_and_uniqueness() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    with pytest.raises(InvalidAgentIdError):
        await service.create_agent(agent_id="Bad Agent", name="Bad Agent")

    await service.create_agent(agent_id="valid-agent", name="Valid Agent")

    with pytest.raises(AgentAlreadyExistsError):
        await service.create_agent(agent_id="valid-agent", name="Duplicate")


@pytest.mark.asyncio
async def test_kill_kernel_destroys_and_marks_sessions_dead() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    await service.create_agent(agent_id="test-agent", name="Test Agent")
    session = await service.create_session(
        agent_id="test-agent",
        channel_name="webui",
        client_type=ClientType.WEBUI,
    )
    kernel_session_id = str(session["agent_host_session_id"])

    await service.kill_kernel(kernel_session_id)

    assert kernel_session_id in upstream.destroyed
    sessions = await service.list_sessions()
    assert len(sessions) == 1
    assert sessions[0]["status"] == "dead"


@pytest.mark.asyncio
async def test_kill_kernel_not_found_raises() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    with pytest.raises(KernelNotFoundError):
        await service.kill_kernel("nonexistent")


@pytest.mark.asyncio
async def test_skills_crud_proxies_to_agent_host() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    created = await service.create_skill("my-skill", {"SKILL.md": "# My Skill"})
    assert created["skill_id"] == "my-skill"

    listed = await service.list_skills()
    assert len(listed) == 1
    assert listed[0]["skill_id"] == "my-skill"

    fetched = await service.get_skill("my-skill")
    assert fetched["files"] == {"SKILL.md": "# My Skill"}

    updated = await service.update_skill("my-skill", {"SKILL.md": "# Updated"})
    assert updated["files"] == {"SKILL.md": "# Updated"}

    await service.delete_skill("my-skill")
    assert await service.list_skills() == []
