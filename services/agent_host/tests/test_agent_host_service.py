from __future__ import annotations

from typing import TYPE_CHECKING, Any, cast

import pytest
from agent_host.service import (
    SKILLS_MOUNT_PATHS,
    AgentHost,
    KernelRuntimeSession,
    SessionNotFoundError,
    _summarize_docker_stats,  # pyright: ignore[reportPrivateUsage]
)
from kernel.events import (
    KernelEvent,
    KernelStatus,
    session_end,
    session_start,
    status_event,
    text_delta,
)
from kernel_host.registry import HarnessName

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, AsyncIterator


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
        additional_paths: tuple[str, ...],
        skills: tuple[str, ...] = (),
    ) -> KernelRuntimeSession:
        del skills
        container_name = f"container-{session_id[:8]}"
        self.created.append(
            {
                "session_id": session_id,
                "harness": harness,
                "env": env,
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
        return [
            event
            async for event in self.stream_message(
                session=session,
                message=message,
            )
        ]

    def stream_message(
        self,
        *,
        session: KernelRuntimeSession,
        message: str,
    ) -> AsyncIterator[KernelEvent]:
        container_name = self._session_key(session)
        self.sent.append((container_name, message))
        events = [
            session_start("kernel-session", "stub"),
            status_event(KernelStatus.BUSY),
            text_delta("hello"),
            status_event(KernelStatus.DONE),
            session_end(),
        ]

        async def iterator() -> AsyncIterator[KernelEvent]:
            try:
                for event in events:
                    yield event
            finally:
                self._histories[container_name].append(events)
                self._summaries[container_name] = {
                    "status": "done",
                    "resume_token": "resume-runtime-2",
                }

        return iterator()

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

    async def logs(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> list[str]:
        del session
        return ['{"type":"stub","data":{}}']

    async def container_logs(
        self,
        *,
        session: KernelRuntimeSession,
        tail: int | None,
    ) -> list[str]:
        container_name = self._session_key(session)
        if tail is not None and tail > 0:
            return [f"{container_name} container line {i}" for i in range(tail)][:3]
        return [f"{container_name} container line {i}" for i in range(5)]

    async def stats(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> dict[str, Any] | None:
        del session
        return {
            "cpu_percent": 12.5,
            "memory_usage_bytes": 50_000_000,
            "memory_limit_bytes": 200_000_000,
            "memory_percent": 25.0,
        }

    def container_name(self, *, session: KernelRuntimeSession) -> str | None:
        return self._session_key(session)

    def vscode_url(self, *, session: KernelRuntimeSession) -> str | None:
        return f"http://127.0.0.1/vscode/{self._session_key(session)}"

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
        additional_paths=("/srv/agent",),
    )
    session_id = session["session_id"]

    events = await host.send_message(session_id, "hello")
    history = await host.history(session_id)
    fetched = await host.get_session(session_id)

    assert runtime.created[0]["harness"] is HarnessName.COPILOT_CLI
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
async def test_stream_message_updates_history_and_status() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(harness=HarnessName.ECHO)
    session_id = str(session["session_id"])

    events = [event async for event in host.stream_message(session_id, "hello")]
    fetched = await host.get_session(session_id)

    assert [event.type for event in events] == [
        "session_start",
        "status",
        "text_delta",
        "status",
        "session_end",
    ]
    assert fetched["turns"] == 1
    assert fetched["status"] == "done"


@pytest.mark.asyncio
async def test_stream_message_finalizes_when_consumer_closes_early() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(harness=HarnessName.ECHO)
    session_id = str(session["session_id"])

    stream = cast(
        "AsyncGenerator[KernelEvent]",
        host.stream_message(session_id, "hello"),
    )
    first = await anext(stream)
    await stream.aclose()
    fetched = await host.get_session(session_id)

    assert first.type == "session_start"
    assert fetched["turns"] == 1
    assert fetched["status"] == "done"


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


def test_skills_mount_paths_covers_all_harnesses() -> None:
    for harness in HarnessName:
        assert harness in SKILLS_MOUNT_PATHS, (
            f"missing SKILLS_MOUNT_PATHS entry for {harness!r}"
        )


def test_copilot_skills_mount_path() -> None:
    assert SKILLS_MOUNT_PATHS[HarnessName.COPILOT_CLI] == "/root/.copilot/skills"


def test_opencode_skills_mount_path() -> None:
    assert SKILLS_MOUNT_PATHS[HarnessName.OPENCODE] == "/root/.config/opencode/skills"


def test_acp_skills_mount_path() -> None:
    assert SKILLS_MOUNT_PATHS[HarnessName.ACP] == "/workspace/.agents/skills"


def test_summarize_docker_stats_computes_percentages() -> None:
    raw: dict[str, Any] = {
        "cpu_stats": {
            "cpu_usage": {"total_usage": 200},
            "system_cpu_usage": 1000,
            "online_cpus": 2,
        },
        "precpu_stats": {
            "cpu_usage": {"total_usage": 100},
            "system_cpu_usage": 500,
        },
        "memory_stats": {
            "usage": 200,
            "limit": 1000,
            "stats": {"cache": 50},
        },
    }

    summary = _summarize_docker_stats(raw)

    assert summary is not None
    cpu_percent = summary["cpu_percent"]
    memory_percent = summary["memory_percent"]
    assert isinstance(cpu_percent, float)
    assert isinstance(memory_percent, float)
    assert abs(cpu_percent - 40.0) < 1e-6
    assert summary["memory_usage_bytes"] == 150
    assert summary["memory_limit_bytes"] == 1000
    assert abs(memory_percent - 15.0) < 1e-6


def test_summarize_docker_stats_handles_missing_fields() -> None:
    summary = _summarize_docker_stats({})

    assert summary is None


def test_summarize_docker_stats_uses_cgroup_v2_inactive_file() -> None:
    raw: dict[str, Any] = {
        "memory_stats": {
            "usage": 200,
            "limit": 1000,
            "stats": {"inactive_file": 75},
        },
    }

    summary = _summarize_docker_stats(raw)

    assert summary is not None
    assert summary["memory_usage_bytes"] == 125
    assert summary["cpu_percent"] is None


@pytest.mark.asyncio
async def test_get_session_includes_container_name_and_stats() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(harness=HarnessName.ECHO)
    fetched = await host.get_session(session["session_id"], with_stats=True)

    assert fetched["container_name"] == f"container-{session['session_id'][:8]}"
    stats = fetched["stats"]
    assert isinstance(stats, dict)
    assert stats["cpu_percent"] == 12.5
    assert stats["memory_usage_bytes"] == 50_000_000


@pytest.mark.asyncio
async def test_get_session_omits_stats_by_default() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(harness=HarnessName.ECHO)
    fetched = await host.get_session(session["session_id"])

    assert fetched["stats"] is None


@pytest.mark.asyncio
async def test_list_sessions_returns_cached_summary_on_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(harness=HarnessName.ECHO)

    async def boom(**_kwargs: object) -> dict[str, Any]:
        msg = "kernel unreachable"
        raise RuntimeError(msg)

    monkeypatch.setattr(runtime, "summary", boom)

    summaries = await host.list_sessions()

    assert len(summaries) == 1
    assert summaries[0]["session_id"] == session["session_id"]


@pytest.mark.asyncio
async def test_container_logs_default_tail_passed_through() -> None:
    runtime = StubRuntime()
    host = AgentHost(runtime=runtime)

    session = await host.create_session(harness=HarnessName.ECHO)
    lines = await host.container_logs(session["session_id"], tail=None)

    assert len(lines) == 5
