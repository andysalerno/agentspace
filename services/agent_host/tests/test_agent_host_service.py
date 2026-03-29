from __future__ import annotations

from typing import Any

import pytest
from agent_host.service import AgentHost, KernelRuntimeSession, SessionNotFoundError
from kernel.events import (
    KernelEvent,
    KernelStatus,
    session_end,
    session_start,
    status_event,
    text_delta,
)
from kernel_host.registry import HarnessName


class StubRuntime:
    def __init__(self) -> None:
        self.created: list[dict[str, Any]] = []
        self.destroyed: list[str] = []
        self.sent: list[tuple[str, str]] = []
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
        container_name = f"container-{session_id[:8]}"
        self.created.append(
            {
                "session_id": session_id,
                "harness": harness,
                "env": env,
                "cwd": cwd,
                "additional_paths": additional_paths,
            },
        )
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
        self.sent.append((container_name, message))
        events = [
            session_start("kernel-session", "stub"),
            status_event(KernelStatus.BUSY),
            text_delta("hello"),
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
        self.destroyed.append(self._session_key(session))

    def _session_key(self, session: KernelRuntimeSession) -> str:
        assert isinstance(session.value, str)
        return session.value


@pytest.mark.asyncio
async def test_create_send_history_and_destroy() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(
        harness=HarnessName.COPILOT_CLI,
        env={"COPILOT_MODEL": "gpt-5.2"},
        cwd="/srv/agent",
        additional_paths=("/srv/agent",),
    )
    session_id = session["session_id"]

    events = await host.send_message(session_id, "hello")
    history = await host.history(session_id)
    fetched = await host.get_session(session_id)

    assert runtime.created[0]["harness"] is HarnessName.COPILOT_CLI
    assert runtime.created[0]["cwd"] == "/srv/agent"
    assert runtime.created[0]["additional_paths"] == ("/srv/agent",)
    assert runtime.created[0]["env"]["COPILOT_MODEL"] == "gpt-5.2"
    assert len(runtime.sent) == 1
    assert runtime.sent[0][1] == "hello"
    assert [event.type for event in events] == [
        "session_start",
        "status",
        "text_delta",
        "status",
        "session_end",
    ]
    assert len(history) == 1
    assert fetched["resume_token"].startswith("resume-runtime-")
    assert fetched["turns"] == 1

    await host.destroy_session(session_id)
    assert len(runtime.destroyed) == 1


@pytest.mark.asyncio
async def test_reset_session_recreates_container() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(harness=HarnessName.COPILOT_CLI)
    new_session = await host.reset_session(session["session_id"])

    assert len(runtime.destroyed) == 1
    assert new_session["session_id"] != session["session_id"]


@pytest.mark.asyncio
async def test_missing_session_raises() -> None:
    host = AgentHost(runtime=StubRuntime())

    with pytest.raises(SessionNotFoundError):
        await host.get_session("missing")
