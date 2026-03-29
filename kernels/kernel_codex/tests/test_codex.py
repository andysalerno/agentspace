"""Tests for CodexKernel event mapping.

Uses the real Codex CLI JSON output format to verify that
``_map_event`` produces the correct standardized kernel events.
"""
# pyright: reportPrivateUsage=false

import json

import pytest
from kernel.events import EventType, KernelEvent, KernelStatus
from kernel.protocol import KernelConfig
from kernel_codex import CodexKernel

# ---------------------------------------------------------------------------
# Sample JSONL lines from a real `codex exec --json --full-auto` run
# ---------------------------------------------------------------------------

SAMPLE_LINES: list[str] = [
    '{"type":"thread.started","thread_id":"019d386d-2f4a-7833-8223-b6d2732a478b"}',
    '{"type":"turn.started"}',
    '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Checking the current working directory and listing its contents."}}',
    '{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"ls -la","aggregated_output":"","exit_code":null,"status":"in_progress"}}',
    '{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"ls -la","aggregated_output":"total 4\\ndrwxr-xr-x 2 user user 4096 Mar 28 20:00 .\\n","exit_code":0,"status":"completed"}}',
    '{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Here are the files in the current directory."}}',
    '{"type":"turn.completed","usage":{"input_tokens":22908,"cached_input_tokens":18176,"output_tokens":307}}',
]

SIMPLE_HELLO_LINES: list[str] = [
    '{"type":"thread.started","thread_id":"019d386d-2f4a-7833-8223-b6d2732a478b"}',
    '{"type":"turn.started"}',
    '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"hello"}}',
    '{"type":"turn.completed","usage":{"input_tokens":11294,"cached_input_tokens":9728,"output_tokens":36}}',
]


async def _drain(kernel: CodexKernel) -> list[KernelEvent]:
    """Drain all queued events without blocking forever."""
    events: list[KernelEvent] = []
    while not kernel._queue.empty():
        evt = kernel._queue.get_nowait()
        if evt is not None:
            events.append(evt)
    return events


class TestCodexMapping:
    """Verify _map_event correctly translates Codex CLI JSON → kernel events."""

    @pytest.fixture
    def kernel(self) -> CodexKernel:
        k = CodexKernel()
        k._session_id = "test-session"
        return k

    @pytest.mark.asyncio
    async def test_thread_started_captures_session_id(
        self,
        kernel: CodexKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[0])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 0
        assert kernel._session_id == "019d386d-2f4a-7833-8223-b6d2732a478b"

    @pytest.mark.asyncio
    async def test_turn_started_produces_no_events(
        self,
        kernel: CodexKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[1])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_agent_message_produces_text_delta(
        self,
        kernel: CodexKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[2])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.TEXT_DELTA
        assert (
            events[0].content
            == "Checking the current working directory and listing its contents."
        )

    @pytest.mark.asyncio
    async def test_command_execution_started_produces_tool_call(
        self,
        kernel: CodexKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[3])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.TOOL_CALL
        assert events[0].tool == "shell"
        assert events[0].input == {"cmd": "ls -la"}

    @pytest.mark.asyncio
    async def test_command_execution_completed_produces_tool_result(
        self,
        kernel: CodexKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[4])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.TOOL_RESULT
        assert events[0].tool == "shell"
        assert "total 4" in (events[0].output or "")
        assert "[exit_code: 0]" in (events[0].output or "")

    @pytest.mark.asyncio
    async def test_turn_completed_produces_idle_status(
        self,
        kernel: CodexKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[6])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.STATUS
        assert events[0].status == KernelStatus.IDLE

    @pytest.mark.asyncio
    async def test_full_simple_conversation(
        self,
        kernel: CodexKernel,
    ) -> None:
        for line in SIMPLE_HELLO_LINES:
            await kernel._map_event(json.loads(line))

        events = await _drain(kernel)
        types = [e.type for e in events]
        assert EventType.TEXT_DELTA in types
        assert EventType.STATUS in types

        text_events = [e for e in events if e.type == EventType.TEXT_DELTA]
        assert text_events[0].content == "hello"

    @pytest.mark.asyncio
    async def test_full_tool_conversation(
        self,
        kernel: CodexKernel,
    ) -> None:
        for line in SAMPLE_LINES:
            await kernel._map_event(json.loads(line))

        events = await _drain(kernel)
        types = [e.type for e in events]
        assert EventType.TEXT_DELTA in types
        assert EventType.TOOL_CALL in types
        assert EventType.TOOL_RESULT in types
        assert EventType.STATUS in types

    @pytest.mark.asyncio
    async def test_build_command_basic(
        self,
        kernel: CodexKernel,
    ) -> None:
        cmd = kernel._build_command("hello world")
        assert cmd[0] == "codex"
        assert cmd[1] == "exec"
        assert "hello world" in cmd
        assert "--json" in cmd
        assert "--full-auto" in cmd

    @pytest.mark.asyncio
    async def test_build_command_with_resume(
        self,
        kernel: CodexKernel,
    ) -> None:
        kernel._config = KernelConfig(session_id="abc-123")
        cmd = kernel._build_command("continue")
        assert "resume" in cmd
        assert "abc-123" in cmd
        assert "continue" in cmd

    @pytest.mark.asyncio
    async def test_build_command_with_model(
        self,
        kernel: CodexKernel,
    ) -> None:
        kernel._config = KernelConfig(env={"CODEX_MODEL": "o3"})
        cmd = kernel._build_command("test")
        assert "--model" in cmd
        idx = cmd.index("--model")
        assert cmd[idx + 1] == "o3"
