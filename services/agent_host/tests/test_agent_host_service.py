from __future__ import annotations

from typing import Any

import pytest
from agent_host.service import AgentHost, SessionNotFoundError
from kernel.events import (
    KernelEvent,
    KernelStatus,
    session_end,
    session_start,
    status_event,
    text_delta,
)


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
        harness: str,
        env: dict[str, str],
        cwd: str | None,
        additional_paths: tuple[str, ...],
    ) -> tuple[str, str]:
        container_name = f"container-{session_id[:8]}"
        base_url = f"http://{container_name}:8000"
        self.created.append(
            {
                "session_id": session_id,
                "harness": harness,
                "env": env,
                "cwd": cwd,
                "additional_paths": additional_paths,
            },
        )
        self._summaries[base_url] = {
            "status": "idle",
            "resume_token": "resume-runtime-1",
        }
        self._histories[base_url] = []
        return container_name, base_url

    async def send_message(self, *, base_url: str, message: str) -> list[KernelEvent]:
        self.sent.append((base_url, message))
        events = [
            session_start("kernel-session", "stub"),
            status_event(KernelStatus.BUSY),
            text_delta("hello"),
            status_event(KernelStatus.DONE),
            session_end(),
        ]
        self._histories[base_url].append(events)
        self._summaries[base_url] = {
            "status": "done",
            "resume_token": "resume-runtime-2",
        }
        return events

    async def summary(self, *, base_url: str) -> dict[str, object]:
        return dict(self._summaries[base_url])

    async def history(self, *, base_url: str) -> list[list[KernelEvent]]:
        return list(self._histories[base_url])

    async def destroy_session(self, *, container_name: str) -> None:
        self.destroyed.append(container_name)


@pytest.mark.asyncio
async def test_create_send_history_and_destroy() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(
        harness="copilot-cli",
        env={"COPILOT_MODEL": "gpt-5.2"},
        cwd="/srv/agent",
        additional_paths=("/srv/agent",),
    )
    session_id = session["session_id"]

    events = await host.send_message(session_id, "hello")
    history = await host.history(session_id)
    fetched = await host.get_session(session_id)

    assert runtime.created[0]["harness"] == "copilot-cli"
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
    assert runtime.destroyed == [session["container_name"]]


@pytest.mark.asyncio
async def test_reset_session_recreates_container() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(harness="copilot-cli")
    new_session = await host.reset_session(session["session_id"])

    assert runtime.destroyed == [session["container_name"]]
    assert new_session["session_id"] != session["session_id"]


@pytest.mark.asyncio
async def test_missing_session_raises() -> None:
    host = AgentHost(runtime=StubRuntime())

    with pytest.raises(SessionNotFoundError):
        await host.get_session("missing")
