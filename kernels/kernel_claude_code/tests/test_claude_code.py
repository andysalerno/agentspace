"""Tests for ClaudeCodeKernel command building and real trace event mapping."""
# pyright: reportPrivateUsage=false

import pytest
from kernel.events import EventType, KernelEvent, KernelStatus
from kernel.protocol import KernelConfig
from kernel_claude_code import ClaudeCodeKernel

SYSTEM_INIT_LINE: dict[str, object] = {
    "type": "system",
    "subtype": "init",
    "session_id": "c7df57b6-0d8d-4cf3-a72d-9a28682ebd53",
}

THINKING_LINE: dict[str, object] = {
    "type": "assistant",
    "message": {
        "content": [
            {
                "type": "thinking",
                "thinking": "The user wants to find the largest file in the repository.",
            },
        ],
    },
    "session_id": "c7df57b6-0d8d-4cf3-a72d-9a28682ebd53",
}

TEXT_LINE: dict[str, object] = {
    "type": "assistant",
    "message": {
        "content": [
            {
                "type": "text",
                "text": "I'll find the largest file in this repository.\n\n",
            },
        ],
    },
    "session_id": "c7df57b6-0d8d-4cf3-a72d-9a28682ebd53",
}

TOOL_USE_LINE: dict[str, object] = {
    "type": "assistant",
    "message": {
        "content": [
            {
                "type": "tool_use",
                "id": "tool-123",
                "name": "Bash",
                "input": {
                    "command": "find /repo -type f",
                    "description": "Find files",
                },
            },
        ],
    },
    "session_id": "c7df57b6-0d8d-4cf3-a72d-9a28682ebd53",
}

TOOL_RESULT_LINE: dict[str, object] = {
    "type": "user",
    "message": {
        "role": "user",
        "content": [
            {
                "tool_use_id": "tool-123",
                "type": "tool_result",
                "content": "380K\t/repo/clients/webui/dist/assets/index.js",
                "is_error": False,
            },
        ],
    },
    "session_id": "c7df57b6-0d8d-4cf3-a72d-9a28682ebd53",
    "tool_use_result": {
        "stdout": "380K\t/repo/clients/webui/dist/assets/index.js",
        "stderr": "",
        "interrupted": False,
    },
}

RESULT_LINE: dict[str, object] = {
    "type": "result",
    "subtype": "success",
    "is_error": False,
    "result": "The largest file is clients/webui/dist/assets/index.js",
    "session_id": "c7df57b6-0d8d-4cf3-a72d-9a28682ebd53",
}


async def _drain(kernel: ClaudeCodeKernel) -> list[KernelEvent]:
    events: list[KernelEvent] = []
    while not kernel._queue.empty():
        evt = kernel._queue.get_nowait()
        if evt is not None:
            events.append(evt)
    return events


class TestClaudeCodeMapping:
    @pytest.fixture
    def kernel(self) -> ClaudeCodeKernel:
        return ClaudeCodeKernel()

    @pytest.mark.asyncio
    async def test_system_init_updates_resume_token_without_emitting_events(
        self,
        kernel: ClaudeCodeKernel,
    ) -> None:
        await kernel._map_event(SYSTEM_INIT_LINE)

        events = await _drain(kernel)
        assert len(events) == 0
        assert kernel.resume_token == "c7df57b6-0d8d-4cf3-a72d-9a28682ebd53"  # noqa: S105

    @pytest.mark.asyncio
    async def test_assistant_thinking_maps_to_reasoning_delta(
        self,
        kernel: ClaudeCodeKernel,
    ) -> None:
        await kernel._map_event(THINKING_LINE)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.REASONING_DELTA
        assert "largest file" in (events[0].content or "")

    @pytest.mark.asyncio
    async def test_assistant_text_maps_to_text_delta(
        self,
        kernel: ClaudeCodeKernel,
    ) -> None:
        await kernel._map_event(TEXT_LINE)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.TEXT_DELTA
        assert "find the largest file" in (events[0].content or "")

    @pytest.mark.asyncio
    async def test_tool_use_and_user_tool_result_map_to_tool_events(
        self,
        kernel: ClaudeCodeKernel,
    ) -> None:
        await kernel._map_event(TOOL_USE_LINE)
        await kernel._map_event(TOOL_RESULT_LINE)

        events = await _drain(kernel)
        assert [event.type for event in events] == [
            EventType.TOOL_CALL,
            EventType.TOOL_RESULT,
        ]
        assert events[0].tool == "Bash"
        assert events[0].input == {
            "command": "find /repo -type f",
            "description": "Find files",
        }
        assert events[1].tool == "Bash"
        assert events[1].output == "380K\t/repo/clients/webui/dist/assets/index.js"

    @pytest.mark.asyncio
    async def test_result_captures_resume_token_and_emits_idle_status(
        self,
        kernel: ClaudeCodeKernel,
    ) -> None:
        await kernel._map_event(RESULT_LINE)

        events = await _drain(kernel)
        assert kernel.resume_token == "c7df57b6-0d8d-4cf3-a72d-9a28682ebd53"  # noqa: S105
        assert len(events) == 1
        assert events[0].type == EventType.STATUS
        assert events[0].status == KernelStatus.IDLE

    @pytest.mark.asyncio
    async def test_real_trace_sequence_emits_expected_events(
        self,
        kernel: ClaudeCodeKernel,
    ) -> None:
        for obj in (
            SYSTEM_INIT_LINE,
            THINKING_LINE,
            TEXT_LINE,
            TOOL_USE_LINE,
            TOOL_RESULT_LINE,
            RESULT_LINE,
        ):
            await kernel._map_event(obj)

        events = await _drain(kernel)
        assert [event.type for event in events] == [
            EventType.REASONING_DELTA,
            EventType.TEXT_DELTA,
            EventType.TOOL_CALL,
            EventType.TOOL_RESULT,
            EventType.STATUS,
        ]
        assert events[-1].status == KernelStatus.IDLE


class TestClaudeCodeLifecycle:
    def test_name(self) -> None:
        assert ClaudeCodeKernel().name == "claude-code"

    def test_initial_status(self) -> None:
        assert ClaudeCodeKernel().status == KernelStatus.IDLE

    @pytest.mark.asyncio
    async def test_start_emits_session_start(self) -> None:
        kernel = ClaudeCodeKernel()

        await kernel.start(KernelConfig())

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.SESSION_START
        assert events[0].kernel == "claude-code"
        assert events[0].session_id is not None

    @pytest.mark.asyncio
    async def test_start_uses_resume_session_id_but_not_as_resume_token_without_capture(
        self,
    ) -> None:
        kernel = ClaudeCodeKernel()

        await kernel.start(KernelConfig(session_id="resume-123"))

        events = await _drain(kernel)
        assert events[0].session_id == "resume-123"
        assert kernel.resume_token == "resume-123"  # noqa: S105


class TestClaudeCodeCommandBuilding:
    def test_build_command_default(self) -> None:
        kernel = ClaudeCodeKernel()
        kernel._config = KernelConfig()

        cmd = kernel._build_command("hello world")

        assert cmd[0] == "claude"
        assert "--print" in cmd
        assert "--bare" in cmd
        assert "--output-format" in cmd
        assert cmd[cmd.index("--output-format") + 1] == "stream-json"
        assert "--tools" in cmd
        assert cmd[cmd.index("--tools") + 1] == (
            "Bash,Read,Edit,Write,Glob,Grep,TodoWrite,Skill,Task"
        )
        assert "--dangerously-skip-permissions" in cmd
        assert cmd[-1] == "hello world"

    def test_build_command_with_resume_effort_and_add_dirs(self) -> None:
        kernel = ClaudeCodeKernel()
        kernel._config = KernelConfig(
            env={
                "CLAUDE_CODE_REASONING_EFFORT": "high",
                "CLAUDE_CODE_ADDITIONAL_PATHS": "/skills:/tmp/other",
                "CLAUDE_CODE_EXTRA_ARGS": "--debug\n--include-partial-messages",
            },
            session_id="resume-123",
            additional_paths=("/repo",),
        )

        cmd = kernel._build_command("continue")

        assert "--effort" in cmd
        assert cmd[cmd.index("--effort") + 1] == "high"
        assert "--resume" in cmd
        assert cmd[cmd.index("--resume") + 1] == "resume-123"
        assert cmd.count("--add-dir") == 3
        assert "--debug" in cmd
        assert "--include-partial-messages" in cmd

    def test_build_command_appends_additional_tools(self) -> None:
        kernel = ClaudeCodeKernel()
        kernel._config = KernelConfig(env={"CLAUDE_CODE_ADDITIONAL_TOOLS": "WebFetch"})

        cmd = kernel._build_command("test")

        tools_value = cmd[cmd.index("--tools") + 1]
        assert tools_value.endswith(",WebFetch")

    def test_build_env_sets_workspace_and_auth_overrides(self) -> None:
        kernel = ClaudeCodeKernel()
        kernel._config = KernelConfig(
            env={
                "CLAUDE_CODE_WORKSPACE_DIR": "/custom/workspace",
                "ANTHROPIC_API_KEY": "secret",
                "ANTHROPIC_BASE_URL": "http://example.invalid",
            },
        )

        env = kernel._build_env()

        assert env["WORKSPACE_DIR"] == "/custom/workspace"
        assert env["ANTHROPIC_API_KEY"] == "secret"
        assert env["ANTHROPIC_BASE_URL"] == "http://example.invalid"
