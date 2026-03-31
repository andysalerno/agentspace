from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

import pytest
from kernel.events import (
    KernelEvent,
    KernelStatus,
    session_end,
    session_start,
    status_event,
    text_delta,
)
from kernel_host.registry import HarnessName
from kernel_host.service import (
    KernelSessionService,
    discover_skill_dirs,
    link_enabled_skills,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from kernel.protocol import KernelConfig


class StubKernel:
    def __init__(self) -> None:
        self.start_configs: list[KernelConfig] = []
        self.messages: list[str] = []
        self.resume_token_value: str | None = None

    @property
    def name(self) -> str:
        return "stub"

    @property
    def status(self) -> KernelStatus:
        return KernelStatus.DONE

    @property
    def resume_token(self) -> str | None:
        return self.resume_token_value

    async def start(self, config: KernelConfig) -> None:
        self.start_configs.append(config)

    async def send(self, message: str) -> None:
        self.messages.append(message)
        if self.resume_token_value is None:
            self.resume_token_value = "resume-kernel-host"  # noqa: S105

    async def recv(self) -> AsyncIterator[KernelEvent]:
        yield session_start("session-1", "stub")
        yield status_event(KernelStatus.BUSY)
        yield text_delta("hello")
        yield status_event(KernelStatus.IDLE)
        yield session_end()

    async def stop(self) -> None:
        return


@pytest.mark.asyncio
async def test_service_reuses_resume_token(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    kernels: list[StubKernel] = []

    def fake_get_kernel(_harness_name: HarnessName) -> StubKernel:
        kernel = StubKernel()
        kernels.append(kernel)
        return kernel

    monkeypatch.setattr("kernel_host.service.get_kernel", fake_get_kernel)

    service = KernelSessionService(
        harness=HarnessName.COPILOT_CLI,
        env={"COPILOT_MODEL": "gpt-5.2"},
        additional_paths=("/srv/kernel",),
    )

    first_events = await service.send_message("hello")
    second_events = await service.send_message("again")
    summary = await service.summary()

    assert len(kernels) == 2
    assert kernels[0].start_configs[0].session_id is None
    assert kernels[1].start_configs[0].session_id == "resume-kernel-host"
    assert [event.type for event in first_events] == [
        "session_start",
        "status",
        "text_delta",
        "status",
        "session_end",
    ]
    assert len(second_events) == 5
    assert summary["resume_token"] == "resume-kernel-host"  # noqa: S105
    assert summary["turns"] == 2


@pytest.mark.asyncio
async def test_stream_message_persists_history_after_iteration(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def fake_get_kernel(_harness_name: HarnessName) -> StubKernel:
        return StubKernel()

    monkeypatch.setattr("kernel_host.service.get_kernel", fake_get_kernel)

    service = KernelSessionService(
        harness=HarnessName.ECHO,
        env={},
        additional_paths=(),
    )

    events = [event async for event in service.stream_message("stream me")]
    history = await service.history()

    assert [event.type for event in events] == [
        "session_start",
        "status",
        "text_delta",
        "status",
        "session_end",
    ]
    assert len(history) == 1
    assert history[0][2].content == "hello"


def test_discover_skill_dirs(tmp_path: object) -> None:
    base = Path(str(tmp_path))
    (base / "alpha-skill").mkdir()
    (base / "beta-skill").mkdir()
    (base / "some-file.txt").write_text("not a dir")

    result = discover_skill_dirs(str(base))

    assert result == (str(base / "alpha-skill"), str(base / "beta-skill"))


def test_discover_skill_dirs_missing_dir() -> None:
    result = discover_skill_dirs("/nonexistent/dir")

    assert result == ()


def test_link_enabled_skills_removes_stale_symlinks(tmp_path: object) -> None:
    base = Path(str(tmp_path))
    staging = base / "staging"
    skills = base / "skills"
    staging.mkdir()
    skills.mkdir()

    # Simulate a prior session that linked "example".
    (staging / "example").mkdir()
    (staging / "other").mkdir()
    (skills / "example").symlink_to(staging / "example")

    # New session enables no skills — stale link should be removed.
    link_enabled_skills(str(staging), str(skills), enabled_skills=set())

    assert not (skills / "example").exists()
    assert not (skills / "other").exists()


def test_link_enabled_skills_keeps_enabled_and_removes_disabled(
    tmp_path: object,
) -> None:
    base = Path(str(tmp_path))
    staging = base / "staging"
    skills = base / "skills"
    staging.mkdir()
    skills.mkdir()

    (staging / "keep-me").mkdir()
    (staging / "drop-me").mkdir()
    # Both were linked in a prior session.
    (skills / "keep-me").symlink_to(staging / "keep-me")
    (skills / "drop-me").symlink_to(staging / "drop-me")

    link_enabled_skills(str(staging), str(skills), enabled_skills={"keep-me"})

    assert (skills / "keep-me").is_symlink()
    assert not (skills / "drop-me").exists()


def test_link_enabled_skills_does_not_remove_non_staging_symlinks(
    tmp_path: object,
) -> None:
    base = Path(str(tmp_path))
    staging = base / "staging"
    skills = base / "skills"
    other = base / "other"
    staging.mkdir()
    skills.mkdir()
    other.mkdir()

    (staging / "staged-skill").mkdir()
    (other / "external").mkdir()
    # A symlink that points outside staging — should not be touched.
    (skills / "external").symlink_to(other / "external")

    link_enabled_skills(str(staging), str(skills), enabled_skills=set())

    assert (skills / "external").is_symlink()
