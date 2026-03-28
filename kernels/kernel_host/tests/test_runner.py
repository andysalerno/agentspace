from __future__ import annotations

import json
from typing import TYPE_CHECKING

import pytest
from kernel.events import (
    KernelEvent,
    KernelStatus,
    session_end,
    session_start,
    status_event,
)
from kernel_host import runner

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from kernel.protocol import KernelConfig


class StubKernel:
    def __init__(self) -> None:
        self.started_config: KernelConfig | None = None
        self.received_message: str | None = None
        self.stopped = False
        self._events = [
            session_start("stub-session", "stub"),
            status_event(KernelStatus.BUSY),
            session_end(),
        ]

    @property
    def name(self) -> str:
        return "stub"

    @property
    def status(self) -> KernelStatus:
        return KernelStatus.IDLE

    async def start(self, config: KernelConfig) -> None:
        self.started_config = config

    async def send(self, message: str) -> None:
        self.received_message = message

    async def recv(self) -> AsyncIterator[KernelEvent]:
        for event in self._events:
            yield event

    async def stop(self) -> None:
        self.stopped = True


@pytest.mark.asyncio
async def test_run_builds_config_from_environment(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    kernel = StubKernel()

    def get_kernel_stub(_harness_name: str) -> StubKernel:
        return kernel

    monkeypatch.setenv("KERNEL_HARNESS", "copilot-cli")
    monkeypatch.setenv("KERNEL_WORKDIR", "/workspace")
    monkeypatch.setenv("KERNEL_SESSION_ID", "resume-123")
    monkeypatch.setenv("KERNEL_ADDITIONAL_PATHS", "/workspace:/workspace-extra")
    monkeypatch.setattr(runner, "get_kernel", get_kernel_stub)

    await runner.run("hello from test")

    assert kernel.received_message == "hello from test"
    assert kernel.stopped is True
    assert kernel.started_config is not None
    assert kernel.started_config.cwd == "/workspace"
    assert kernel.started_config.session_id == "resume-123"
    assert kernel.started_config.additional_paths == (
        "/workspace",
        "/workspace-extra",
    )

    stdout_lines = capsys.readouterr().out.strip().splitlines()
    assert [json.loads(line)["type"] for line in stdout_lines] == [
        "session_start",
        "status",
        "session_end",
    ]
