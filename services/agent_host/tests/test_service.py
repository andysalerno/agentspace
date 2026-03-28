from __future__ import annotations

from typing import TYPE_CHECKING

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

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from kernel.protocol import KernelConfig


class StubKernel:
    def __init__(self) -> None:
        self.started_config: KernelConfig | None = None
        self.sent_messages: list[str] = []
        self.stopped = False
        self.status_value = KernelStatus.IDLE
        self.resume_token_value: str | None = None

    @property
    def name(self) -> str:
        return "stub"

    @property
    def status(self) -> KernelStatus:
        return self.status_value

    @property
    def resume_token(self) -> str | None:
        return self.resume_token_value

    async def start(self, config: KernelConfig) -> None:
        self.started_config = config

    async def send(self, message: str) -> None:
        self.sent_messages.append(message)
        self.status_value = KernelStatus.DONE
        if self.resume_token_value is None:
            self.resume_token_value = "resume-ref-1"  # noqa: S105

    async def recv(self) -> AsyncIterator[KernelEvent]:
        yield session_start("kernel-session", "stub")
        yield status_event(KernelStatus.BUSY)
        yield text_delta("hello")
        yield status_event(KernelStatus.DONE)
        yield session_end()

    async def stop(self) -> None:
        self.stopped = True


@pytest.mark.asyncio
async def test_create_send_history_and_destroy(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    host = AgentHost()
    kernels: list[StubKernel] = []

    def fake_get_kernel(_harness_name: str) -> StubKernel:
        kernel = StubKernel()
        kernels.append(kernel)
        return kernel

    monkeypatch.setattr("agent_host.service.get_kernel", fake_get_kernel)

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

    assert len(kernels) == 1
    assert kernels[0].started_config is not None
    assert kernels[0].started_config.env["COPILOT_MODEL"] == "gpt-5.2"
    assert kernels[0].started_config.cwd == "/srv/agent"
    assert kernels[0].started_config.additional_paths == ("/srv/agent",)
    assert kernels[0].sent_messages == ["hello"]
    assert [event.type for event in events] == [
        "session_start",
        "status",
        "text_delta",
        "status",
        "session_end",
    ]
    assert len(history) == 1
    assert fetched["resume_token"] == "resume-ref-1"  # noqa: S105
    assert fetched["turns"] == 1

    await host.destroy_session(session_id)
    assert kernels[0].stopped is True


@pytest.mark.asyncio
async def test_reset_session_recreates_kernel(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    host = AgentHost()
    kernels: list[StubKernel] = []

    def fake_get_kernel(_harness_name: str) -> StubKernel:
        kernel = StubKernel()
        kernels.append(kernel)
        return kernel

    monkeypatch.setattr("agent_host.service.get_kernel", fake_get_kernel)

    session = await host.create_session(harness="copilot-cli")
    new_session = await host.reset_session(session["session_id"])

    assert len(kernels) == 2
    assert kernels[0].stopped is True
    assert new_session["session_id"] != session["session_id"]


@pytest.mark.asyncio
async def test_missing_session_raises() -> None:
    host = AgentHost()

    with pytest.raises(SessionNotFoundError):
        await host.get_session("missing")
