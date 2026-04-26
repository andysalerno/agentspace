# pyright: reportPrivateUsage=false
"""Tests for ACP kernel event mapping and JSON-RPC helpers."""

from __future__ import annotations

import pytest
from kernel.events import EventType, KernelEvent
from kernel.protocol import KernelConfig
from kernel_acp import AcpKernel


async def _drain(kernel: AcpKernel) -> list[KernelEvent]:
    events: list[KernelEvent] = []
    while not kernel._queue.empty():
        event = kernel._queue.get_nowait()
        if event is not None:
            events.append(event)
    return events


class TestAcpMapping:
    @pytest.fixture
    def kernel(self) -> AcpKernel:
        k = AcpKernel()
        k._session_id = "test-session"
        return k

    @pytest.mark.asyncio
    async def test_agent_message_chunk_produces_text_delta(
        self,
        kernel: AcpKernel,
    ) -> None:
        await kernel._map_session_update(
            {
                "sessionId": "sess_123",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "Hello"},
                },
            },
        )

        events = await _drain(kernel)
        assert kernel._session_id == "sess_123"
        assert len(events) == 1
        assert events[0].type == EventType.TEXT_DELTA
        assert events[0].content == "Hello"

    @pytest.mark.asyncio
    async def test_agent_thought_chunk_produces_reasoning_delta(
        self,
        kernel: AcpKernel,
    ) -> None:
        await kernel._map_session_update(
            {
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "thinking"},
                },
            },
        )

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.REASONING_DELTA
        assert events[0].content == "thinking"

    @pytest.mark.asyncio
    async def test_tool_call_and_completed_update_produce_tool_events(
        self,
        kernel: AcpKernel,
    ) -> None:
        await kernel._map_session_update(
            {
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "call_1",
                    "title": "Run tests",
                    "status": "pending",
                    "rawInput": {"cmd": "pytest"},
                },
            },
        )
        await kernel._map_session_update(
            {
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "call_1",
                    "status": "completed",
                    "content": [
                        {
                            "type": "content",
                            "content": {"type": "text", "text": "passed"},
                        },
                    ],
                },
            },
        )

        events = await _drain(kernel)
        assert len(events) == 2
        assert events[0].type == EventType.TOOL_CALL
        assert events[0].tool == "Run tests"
        assert events[0].input == {"cmd": "pytest"}
        assert events[1].type == EventType.TOOL_RESULT
        assert events[1].tool == "Run tests"
        assert events[1].output == "passed"

    @pytest.mark.asyncio
    async def test_tool_call_update_with_raw_output_produces_result(
        self,
        kernel: AcpKernel,
    ) -> None:
        kernel._tool_names["call_1"] = "Read file"

        await kernel._map_session_update(
            {
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "call_1",
                    "status": "completed",
                    "rawOutput": {"text": "content"},
                },
            },
        )

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.TOOL_RESULT
        assert events[0].tool == "Read file"
        assert events[0].output == '{"text":"content"}'

    def test_build_command_defaults_to_opencode_acp(self, kernel: AcpKernel) -> None:
        assert kernel._build_command() == ["opencode", "acp"]

    def test_build_command_from_env(self, kernel: AcpKernel) -> None:
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_COMMAND": "my-agent --acp",
                "KERNEL_ACP_EXTRA_ARGS": "--debug\n--model=test",
            },
        )

        assert kernel._build_command() == [
            "my-agent",
            "--acp",
            "--debug",
            "--model=test",
        ]

    def test_permission_response_prefers_allow_once(self, kernel: AcpKernel) -> None:
        result = kernel._permission_response(
            {
                "options": [
                    {"optionId": "reject"},
                    {"optionId": "allow_once"},
                ],
            },
        )

        assert result == {
            "outcome": {"outcome": "selected", "optionId": "allow_once"},
        }

    def test_mcp_servers_from_json_env(self, kernel: AcpKernel) -> None:
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_MCP_SERVERS": (
                    '[{"name":"fs","command":"/bin/mcp","args":[],"env":[]}]'
                ),
            },
        )

        assert kernel._mcp_servers() == [
            {"name": "fs", "command": "/bin/mcp", "args": [], "env": []},
        ]

    def test_supports_resume(self, kernel: AcpKernel) -> None:
        kernel._agent_capabilities = {"sessionCapabilities": {"resume": {}}}

        assert kernel._supports_resume() is True
