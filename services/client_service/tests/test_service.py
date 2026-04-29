from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any, Self, cast

import client_service.service as service_module
import pytest
from client_service.models import (
    ClientType,
    MessageRecord,
    MessageRole,
    SessionRecord,
)
from client_service.service import (
    AgentAlreadyExistsError,
    AgentNotFoundError,
    ClientService,
    InvalidAgentIdError,
    KernelNotFoundError,
    SessionNotFoundError,
    parse_env_vars,
)
from client_service.storage import Database, SqliteSessionStore
from gateway.protocol import GatewayType
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
from kernel_host.registry import HarnessName

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, AsyncIterator
    from pathlib import Path

    from client_service.agent_host_client import AgentHostClient


class StubAgentHostClient:
    def __init__(self) -> None:
        self.created: list[dict[str, Any]] = []
        self.sent: list[tuple[str, str]] = []
        self.destroyed: list[str] = []
        self._sessions: dict[str, dict[str, str]] = {}
        self._skills: dict[str, dict[str, object]] = {}
        self.gateways: dict[str, dict[str, object]] = {}
        self.gateway_destroyed: list[str] = []

    async def create_session(
        self,
        *,
        harness: HarnessName,
        skills: list[str] | None = None,
        env: dict[str, str] | None = None,
    ) -> dict[str, object]:
        del skills
        session_id = f"host-{len(self.created) + 1}"
        self.created.append({"harness": harness, "session_id": session_id, "env": env})
        self._sessions[session_id] = {"session_id": session_id, "status": "idle"}
        session: dict[str, object] = {"session_id": session_id, "status": "idle"}
        return session

    async def get_session(self, session_id: str) -> dict[str, object]:
        return {
            "session_id": self._sessions[session_id]["session_id"],
            "status": self._sessions[session_id]["status"],
        }

    async def list_sessions(
        self,
        *,
        with_stats: bool = False,
    ) -> list[dict[str, object]]:
        del with_stats
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

    async def container_logs(
        self,
        session_id: str,
        *,
        tail: int | None,
    ) -> list[str]:
        if session_id not in self._sessions:
            msg = f"session not found: {session_id}"
            raise KeyError(msg)
        lines = [f"container line {i}" for i in range(5)]
        if tail is not None and tail > 0:
            return lines[-tail:]
        return lines

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

    async def info(self) -> dict[str, object]:
        return {
            "service": "agent_host",
            "env_prefix": "AGENT_HOST_",
            "env": {"AGENT_HOST_STUB": "1"},
        }

    async def create_gateway(
        self,
        *,
        gateway_id: str,
        gateway_type: str,
        agent_id: str,
        env: dict[str, str],
    ) -> dict[str, object]:
        record: dict[str, object] = {
            "gateway_id": gateway_id,
            "gateway_type": gateway_type,
            "agent_id": agent_id,
            "container_name": f"agentspace-gateway-{gateway_id}",
            "base_url": f"http://agentspace-gateway-{gateway_id}:8000",
            "env": env,
        }
        self.gateways[gateway_id] = record
        return record

    async def list_gateways(self) -> list[dict[str, object]]:
        return list(self.gateways.values())

    async def get_gateway(self, gateway_id: str) -> dict[str, object]:
        return dict(self.gateways[gateway_id])

    async def gateway_logs(self, gateway_id: str) -> list[str]:
        del gateway_id
        return ["startup", "ok"]

    async def destroy_gateway(self, gateway_id: str) -> None:
        self.gateways.pop(gateway_id, None)
        self.gateway_destroyed.append(gateway_id)


class SlowAgentHostClient(StubAgentHostClient):
    def __init__(self) -> None:
        super().__init__()
        self.release = asyncio.Event()
        self.done = asyncio.Event()

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[KernelEvent]:
        self.sent.append((session_id, message))

        async def iterator() -> AsyncIterator[KernelEvent]:
            yield session_start(session_id, "copilot-cli")
            yield status_event(KernelStatus.BUSY)
            yield text_delta("hel")
            await self.release.wait()
            yield text_delta("lo")
            yield status_event(KernelStatus.DONE)
            yield session_end()
            self._sessions[session_id]["status"] = "done"
            self.done.set()

        return iterator()


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

    assert agent["harness"] == "acp"
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

    assert await service.list_harnesses() == ["acp"]


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
                text_delta("Let me inspect that. "),
                tool_call("read_file", {"path": "src/foo.py"}),
                tool_result("read_file", "print('hello')"),
                text_delta("Now I will write the fix. "),
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

    assert assistant_message["content"] == (
        "Let me inspect that. Now I will write the fix. Done editing."
    )
    assert assistant_message["tool_calls"] == [
        {
            "tool": "read_file",
            "input": '{\n  "path": "src/foo.py"\n}',
            "output": "print('hello')",
            "content_offset": 20,
        },
        {
            "tool": "write_file",
            "input": '{\n  "path": "src/bar.py",\n  "content": "x = 1"\n}',
            "output": "ok",
            "content_offset": 46,
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
            "content_offset": 20,
        },
        {
            "tool": "write_file",
            "input": '{\n  "path": "src/bar.py",\n  "content": "x = 1"\n}',
            "output": "ok",
            "content_offset": 46,
        },
    ]


@pytest.mark.asyncio
async def test_repeated_tool_invocations_pair_outputs_in_order() -> None:
    """Multiple invocations of the same tool should each retain their output."""

    class RepeatedToolStub(StubAgentHostClient):
        def stream_message(
            self,
            session_id: str,
            message: str,
        ) -> AsyncIterator[KernelEvent]:
            self.sent.append((session_id, message))
            events = [
                session_start(session_id, "copilot-cli"),
                status_event(KernelStatus.BUSY),
                tool_call("read_file", {"path": "a.py"}),
                tool_result("read_file", "contents-a"),
                tool_call("read_file", {"path": "b.py"}),
                tool_result("read_file", "contents-b"),
                tool_call("read_file", {"path": "c.py"}),
                tool_result("read_file", ""),
                text_delta("done"),
                status_event(KernelStatus.DONE),
                session_end(),
            ]

            async def iterator() -> AsyncIterator[KernelEvent]:
                for event in events:
                    yield event
                self._sessions[session_id]["status"] = "done"

            return iterator()

    upstream = RepeatedToolStub()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))
    agent = await service.create_agent(agent_id="repeat-agent", name="Repeat Agent")
    session = await service.create_session(agent_id=str(agent["agent_id"]))
    session_id = str(session["session_id"])

    reply = await service.send_message(session_id, "read several files")
    assistant_message = cast("dict[str, object]", reply["assistant_message"])

    assert assistant_message["tool_calls"] == [
        {
            "tool": "read_file",
            "input": '{\n  "path": "a.py"\n}',
            "output": "contents-a",
            "content_offset": 0,
        },
        {
            "tool": "read_file",
            "input": '{\n  "path": "b.py"\n}',
            "output": "contents-b",
            "content_offset": 0,
        },
        {
            "tool": "read_file",
            "input": '{\n  "path": "c.py"\n}',
            "output": "",
            "content_offset": 0,
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
async def test_stream_message_continues_after_client_closes() -> None:
    upstream = SlowAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    agent = await service.create_agent(agent_id="slow-agent", name="Slow Agent")
    session = await service.create_session(agent_id=str(agent["agent_id"]))
    session_id = str(session["session_id"])

    stream = cast(
        "AsyncGenerator[dict[str, object]]",
        service.stream_message(session_id, "hello"),
    )
    first_chunks: list[dict[str, object]] = []
    while True:
        chunk = await anext(stream)
        first_chunks.append(chunk)
        event = cast("dict[str, object]", chunk.get("event", {}))
        if event.get("type") == "text_delta":
            break

    await stream.aclose()
    upstream.release.set()
    await asyncio.wait_for(upstream.done.wait(), timeout=1.0)

    messages = await service.list_messages(session_id)

    assert [chunk["type"] for chunk in first_chunks] == ["event", "event", "event"]
    assert messages[0]["content"] == "hello"
    assert messages[1]["content"] == "hello"


@pytest.mark.asyncio
async def test_stream_turn_attaches_to_running_turn_after_disconnect() -> None:
    upstream = SlowAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    agent = await service.create_agent(agent_id="reconnect-agent", name="Reconnect")
    session = await service.create_session(agent_id=str(agent["agent_id"]))
    session_id = str(session["session_id"])

    stream = cast(
        "AsyncGenerator[dict[str, object]]",
        service.stream_message(session_id, "hello"),
    )
    while True:
        chunk = await anext(stream)
        event = cast("dict[str, object]", chunk.get("event", {}))
        if event.get("type") == "text_delta":
            break
    await stream.aclose()

    detail = await service.get_session(session_id)
    active_turn = cast("dict[str, object]", detail["active_turn"])
    messages = cast("list[dict[str, object]]", detail["messages"])

    assert active_turn["assistant_message_id"] == messages[1]["message_id"]
    assert messages[1]["content"] == "hel"

    attached_stream = service.stream_turn(session_id, str(active_turn["turn_id"]))
    upstream.release.set()
    attached_chunks = [chunk async for chunk in attached_stream]

    assert attached_chunks[-1]["type"] == "final"
    assert (
        cast("dict[str, object]", attached_chunks[-1]["assistant_message"])["content"]
        == "hello"
    )


@pytest.mark.asyncio
async def test_sqlite_session_store_persists_messages(tmp_path: Path) -> None:
    database = Database(str(tmp_path / "client.db"))
    await database.connect()
    store = SqliteSessionStore(database)
    await store.initialize()

    session = SessionRecord(
        session_id="session-1",
        agent_id="agent-one",
        agent_host_session_id="host-1",
        status="idle",
        channel_name="webui",
        client_type=ClientType.WEBUI,
    )
    await store.insert(session)
    await store.append_message(
        MessageRecord(
            message_id="msg-1",
            session_id="session-1",
            role=MessageRole.USER,
            content="hello",
        ),
    )
    await store.append_message(
        MessageRecord(
            message_id="msg-2",
            session_id="session-1",
            role=MessageRole.ASSISTANT,
            content="hi",
        ),
    )

    loaded = await store.get("session-1")
    await database.close()

    assert loaded is not None
    assert loaded.summary()["message_count"] == 2
    assert [message.content for message in loaded.messages] == ["hello", "hi"]


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
async def test_create_session_merges_kernel_config_env_with_agent_env() -> None:
    upstream = StubAgentHostClient()
    runtime = cast("AgentHostClient", upstream)
    service = ClientService(agent_host_client=runtime)

    await service.update_kernel_config(
        HarnessName.OPENCODE,
        "KERNEL_OPENCODE_BASE_URL=https://example.test/v1\n"
        "KERNEL_OPENCODE_API_KEY=from-kernel-config\n"
        "KERNEL_OPENCODE_MODEL_NAME=base-model\n",
    )
    agent = await service.create_agent(
        agent_id="opencode-agent",
        name="OpenCode Agent",
        harness=HarnessName.OPENCODE,
        env_vars="KERNEL_OPENCODE_API_KEY=from-agent\nEXTRA=1\n",
    )

    await service.create_session(agent_id=str(agent["agent_id"]))

    assert len(upstream.created) == 1
    env = cast("dict[str, str]", upstream.created[0]["env"])
    # per-harness defaults are present
    assert env["KERNEL_OPENCODE_BASE_URL"] == "https://example.test/v1"
    assert env["KERNEL_OPENCODE_MODEL_NAME"] == "base-model"
    # per-agent overrides per-harness
    assert env["KERNEL_OPENCODE_API_KEY"] == "from-agent"
    # per-agent extras pass through
    assert env["EXTRA"] == "1"


@pytest.mark.asyncio
async def test_create_session_adds_agent_connection_env() -> None:
    upstream = StubAgentHostClient()
    runtime = cast("AgentHostClient", upstream)
    service = ClientService(agent_host_client=runtime)

    await service.create_connection(
        connection_id="test-connection",
        name="Test Connection",
        url="https://connection.test/v1",
        api_key="connection-secret",
    )
    agent = await service.create_agent(
        agent_id="connected-agent",
        name="Connected Agent",
        harness=HarnessName.OPENCODE,
        connection_id="test-connection",
    )

    await service.create_session(agent_id=str(agent["agent_id"]))

    assert len(upstream.created) == 1
    env = cast("dict[str, str]", upstream.created[0]["env"])
    assert env["CONNECTION_URL"] == "https://connection.test/v1"
    assert env["CONNECTION_API_KEY"] == "connection-secret"


@pytest.mark.asyncio
async def test_create_acp_session_adds_connection_and_model_env() -> None:
    upstream = StubAgentHostClient()
    runtime = cast("AgentHostClient", upstream)
    service = ClientService(agent_host_client=runtime)

    await service.update_kernel_config(
        HarnessName.ACP,
        "KERNEL_ACP_MODEL_NAME=base-model\n",
    )
    await service.create_connection(
        connection_id="test-connection",
        name="Test Connection",
        url="https://connection.test/v1",
        api_key="connection-secret",
    )
    agent = await service.create_agent(
        agent_id="connected-acp-agent",
        name="Connected ACP Agent",
        harness=HarnessName.ACP,
        connection_id="test-connection",
    )

    await service.create_session(agent_id=str(agent["agent_id"]))

    assert len(upstream.created) == 1
    created = upstream.created[0]
    assert created["harness"] == HarnessName.ACP
    env = cast("dict[str, str]", created["env"])
    assert env["CONNECTION_URL"] == "https://connection.test/v1"
    assert env["CONNECTION_API_KEY"] == "connection-secret"
    assert env["KERNEL_ACP_MODEL_NAME"] == "base-model"


@pytest.mark.asyncio
async def test_list_connection_models_fetches_models_with_connection_auth(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    requested: dict[str, object] = {}

    class FakeResponse:
        def raise_for_status(self) -> None:
            return None

        def json(self) -> dict[str, object]:
            return {
                "object": "list",
                "data": [{"id": "model-a", "object": "model"}],
            }

    class FakeAsyncClient:
        def __init__(self, *, timeout: float) -> None:
            requested["timeout"] = timeout

        async def __aenter__(self) -> Self:
            return self

        async def __aexit__(
            self,
            exc_type: object,
            exc: object,
            traceback: object,
        ) -> None:
            return None

        async def get(
            self,
            url: str,
            *,
            headers: dict[str, str],
        ) -> FakeResponse:
            requested["url"] = url
            requested["headers"] = headers
            return FakeResponse()

    monkeypatch.setattr(service_module.httpx, "AsyncClient", FakeAsyncClient)
    service = ClientService(
        agent_host_client=cast("AgentHostClient", StubAgentHostClient()),
    )
    await service.create_connection(
        connection_id="test-connection",
        name="Test Connection",
        url="https://connection.test/v1/",
        api_key="connection-secret",
    )

    models = await service.list_connection_models("test-connection")

    assert models == {
        "object": "list",
        "data": [{"id": "model-a", "object": "model"}],
    }
    assert requested["url"] == "https://connection.test/v1/models"
    assert requested["headers"] == {"Authorization": "Bearer connection-secret"}


@pytest.mark.asyncio
async def test_missing_records_raise() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    with pytest.raises(AgentNotFoundError):
        await service.create_session(agent_id="missing")

    with pytest.raises(SessionNotFoundError):
        await service.send_message("missing", "hello")


@pytest.mark.asyncio
async def test_info_aggregates_and_filters_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("CLIENT_SERVICE_FOO", "foo-value")
    monkeypatch.setenv("UNRELATED_VAR", "should-not-appear")
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    payload = await service.info()

    client_section = cast("dict[str, object]", payload["client_service"])
    client_env = cast("dict[str, str]", client_section["env"])
    assert client_section["service"] == "client_service"
    assert client_section["env_prefix"] == "CLIENT_SERVICE_"
    assert client_env["CLIENT_SERVICE_FOO"] == "foo-value"
    assert "UNRELATED_VAR" not in client_env

    agent_host_section = cast("dict[str, object]", payload["agent_host"])
    assert agent_host_section["service"] == "agent_host"
    assert "error" not in agent_host_section
    assert "webui" not in payload


@pytest.mark.asyncio
async def test_info_degrades_gracefully_when_agent_host_fails() -> None:
    class FailingClient(StubAgentHostClient):
        async def info(self) -> dict[str, object]:
            msg = "boom"
            raise RuntimeError(msg)

    runtime = cast("AgentHostClient", FailingClient())
    service = ClientService(agent_host_client=runtime)

    payload = await service.info()

    agent_host_section = cast("dict[str, object]", payload["agent_host"])
    assert agent_host_section["service"] == "agent_host"
    assert agent_host_section["error"] == "boom"
    # client_service section should still be present even when upstream fails.
    assert "client_service" in payload


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
async def test_kernel_container_logs_proxies_to_agent_host() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    await service.create_agent(agent_id="test-agent", name="Test Agent")
    session = await service.create_session(
        agent_id="test-agent",
        channel_name="webui",
        client_type=ClientType.WEBUI,
    )
    kernel_session_id = str(session["agent_host_session_id"])

    lines = await service.kernel_container_logs(kernel_session_id, tail=2)

    assert lines == ["container line 3", "container line 4"]


@pytest.mark.asyncio
async def test_kernel_container_logs_not_found_raises() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    with pytest.raises(KernelNotFoundError):
        await service.kernel_container_logs("nonexistent", tail=None)


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


@pytest.mark.asyncio
async def test_env_vars_stored_on_agent() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    env_text = "API_KEY=sk-123\nMODEL=claude-sonnet"
    agent = await service.create_agent(
        agent_id="env-agent",
        name="Env Agent",
        env_vars=env_text,
    )
    assert agent["env_vars"] == env_text

    fetched = await service.get_agent("env-agent")
    assert fetched["env_vars"] == env_text


@pytest.mark.asyncio
async def test_env_vars_updated_on_agent() -> None:
    runtime = cast("AgentHostClient", StubAgentHostClient())
    service = ClientService(agent_host_client=runtime)

    await service.create_agent(
        agent_id="env-agent",
        name="Env Agent",
        env_vars="OLD_KEY=old",
    )
    updated = await service.update_agent(
        "env-agent",
        name=None,
        harness=None,
        system_prompt=None,
        skills=None,
        env_vars="NEW_KEY=new",
    )
    assert updated["env_vars"] == "NEW_KEY=new"


@pytest.mark.asyncio
async def test_env_vars_passed_to_agent_host_on_session_create() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    env_text = "API_KEY=sk-123\nMODEL=claude-sonnet"
    await service.create_agent(
        agent_id="env-agent",
        name="Env Agent",
        env_vars=env_text,
    )
    await service.create_session(agent_id="env-agent")

    assert len(upstream.created) == 1
    assert upstream.created[0]["env"] == {
        "API_KEY": "sk-123",
        "MODEL": "claude-sonnet",
    }


@pytest.mark.asyncio
async def test_empty_env_vars_passes_empty_dict() -> None:
    upstream = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", upstream))

    await service.create_agent(agent_id="no-env", name="No Env")
    await service.create_session(agent_id="no-env")

    assert upstream.created[0]["env"] == {}


def test_parse_env_vars_basic() -> None:
    result = parse_env_vars("KEY=value\nFOO=bar")
    assert result == {"KEY": "value", "FOO": "bar"}


def test_parse_env_vars_comments_and_blanks() -> None:
    result = parse_env_vars("# comment\nKEY=value\n\n# another comment\nFOO=bar\n")
    assert result == {"KEY": "value", "FOO": "bar"}


def test_parse_env_vars_quoted_values() -> None:
    result = parse_env_vars("SINGLE='hello world'\nDOUBLE=\"hello world\"")
    assert result == {"SINGLE": "hello world", "DOUBLE": "hello world"}


def test_parse_env_vars_empty_string() -> None:
    assert parse_env_vars("") == {}


def test_parse_env_vars_value_with_equals() -> None:
    result = parse_env_vars("URL=https://example.com?foo=bar&baz=qux")
    assert result == {"URL": "https://example.com?foo=bar&baz=qux"}


@pytest.mark.asyncio
async def test_gateway_lifecycle() -> None:
    runtime = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", runtime))
    await service.create_agent(agent_id="agent-one", name="Agent One")

    created = await service.create_gateway(
        gateway_id="echo-bridge",
        name="Echo Bridge",
        gateway_type=GatewayType.ECHO,
        agent_id="agent-one",
        enabled=True,
        env_vars="ECHO_TOKEN=abc",
        secrets={"DISCORD_TOKEN": "secret"},
    )

    assert created["status"] == "running"
    assert created["secret_keys"] == ["DISCORD_TOKEN"]
    assert "secrets" not in created
    assert "echo-bridge" in runtime.gateways
    assert runtime.gateways["echo-bridge"]["env"] == {
        "ECHO_TOKEN": "abc",
        "DISCORD_TOKEN": "secret",
    }

    listed = await service.list_gateways()
    assert len(listed) == 1
    assert listed[0]["gateway_id"] == "echo-bridge"

    logs = await service.gateway_logs("echo-bridge")
    assert logs == ["startup", "ok"]

    await service.delete_gateway("echo-bridge")
    assert "echo-bridge" not in runtime.gateways
    assert await service.list_gateways() == []


@pytest.mark.asyncio
async def test_update_gateway_secrets_overlay_preserves_existing() -> None:
    """Passing `secrets={NEW: ...}` to update overlays — it does NOT wipe.

    This is the contract the WebUI relies on so a user can edit one
    secret without re-entering every other one (which the API never
    returns to them).
    """
    runtime = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", runtime))
    await service.create_agent(agent_id="agent-one", name="Agent One")
    await service.create_gateway(
        gateway_id="bridge",
        name="Bridge",
        gateway_type=GatewayType.ECHO,
        agent_id="agent-one",
        enabled=False,
        secrets={"TOKEN_A": "alpha", "TOKEN_B": "beta"},
    )

    updated = await service.update_gateway(
        "bridge",
        secrets={"TOKEN_B": "beta-rotated"},
    )

    # Both keys are still present; only TOKEN_B was rotated.
    assert sorted(cast("list[str]", updated["secret_keys"])) == [
        "TOKEN_A",
        "TOKEN_B",
    ]
    inner = await service._require_gateway("bridge")  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    assert inner.secrets == {"TOKEN_A": "alpha", "TOKEN_B": "beta-rotated"}


@pytest.mark.asyncio
async def test_update_gateway_restarts_running_when_config_changes() -> None:
    """Editing env_vars on a running gateway tears down + respawns it.

    Verified by checking that the env the runtime sees post-update
    reflects the new value, which the stub only refreshes on
    create_gateway.
    """
    runtime = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", runtime))
    await service.create_agent(agent_id="agent-one", name="Agent One")
    await service.create_gateway(
        gateway_id="bridge",
        name="Bridge",
        gateway_type=GatewayType.ECHO,
        agent_id="agent-one",
        enabled=True,
        env_vars="MODE=initial",
    )
    assert runtime.gateways["bridge"]["env"] == {"MODE": "initial"}
    assert runtime.gateway_destroyed == []

    updated = await service.update_gateway(
        "bridge",
        env_vars="MODE=updated\nEXTRA=1",
    )

    assert updated["status"] == "running"
    assert runtime.gateway_destroyed == ["bridge"]
    assert runtime.gateways["bridge"]["env"] == {"MODE": "updated", "EXTRA": "1"}


@pytest.mark.asyncio
async def test_update_gateway_no_restart_when_only_metadata_changes() -> None:
    """Renaming a running gateway does not bounce its container."""
    runtime = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", runtime))
    await service.create_agent(agent_id="agent-one", name="Agent One")
    await service.create_gateway(
        gateway_id="bridge",
        name="Bridge",
        gateway_type=GatewayType.ECHO,
        agent_id="agent-one",
        enabled=True,
        env_vars="MODE=stable",
    )
    assert runtime.gateway_destroyed == []

    await service.update_gateway("bridge", name="Renamed Bridge")

    assert runtime.gateway_destroyed == []  # no respawn
    inner = await service._require_gateway("bridge")  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    assert inner.name == "Renamed Bridge"


@pytest.mark.asyncio
async def test_update_gateway_no_restart_when_stopped() -> None:
    """Editing config on a stopped gateway just persists; no spurious start."""
    runtime = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", runtime))
    await service.create_agent(agent_id="agent-one", name="Agent One")
    await service.create_gateway(
        gateway_id="bridge",
        name="Bridge",
        gateway_type=GatewayType.ECHO,
        agent_id="agent-one",
        enabled=False,
        env_vars="MODE=initial",
    )
    assert "bridge" not in runtime.gateways

    await service.update_gateway("bridge", env_vars="MODE=updated")

    assert "bridge" not in runtime.gateways  # still not started


@pytest.mark.asyncio
async def test_gateway_autostart_starts_enabled_only() -> None:
    runtime = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", runtime))
    await service.create_agent(agent_id="agent-one", name="Agent One")
    await service.create_gateway(
        gateway_id="enabled-one",
        name="Enabled",
        gateway_type=GatewayType.ECHO,
        agent_id="agent-one",
        enabled=False,
    )
    record = await service.update_gateway("enabled-one", enabled=False)
    assert record["status"] == "stopped"
    runtime.gateways.clear()

    # Manually flip the persisted record to enabled (skipping start_gateway)
    # then run autostart and verify the runtime container is recreated.
    inner_record = await service._require_gateway("enabled-one")  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    inner_record.enabled = True
    await service._gateway_store.update(inner_record)  # type: ignore[reportPrivateUsage]  # noqa: SLF001

    await service.autostart_enabled_gateways()
    assert "enabled-one" in runtime.gateways


@pytest.mark.asyncio
async def test_gateway_create_unknown_agent_raises() -> None:
    runtime = StubAgentHostClient()
    service = ClientService(agent_host_client=cast("AgentHostClient", runtime))
    with pytest.raises(AgentNotFoundError):
        await service.create_gateway(
            gateway_id="echo-bridge",
            name="Echo Bridge",
            gateway_type=GatewayType.ECHO,
            agent_id="missing",
        )
