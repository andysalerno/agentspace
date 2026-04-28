"""Tests for OpenCodeKernel event mapping.

Uses the real OpenCode CLI JSON output format to verify that
``_map_event`` produces the correct standardized kernel events.
"""
# pyright: reportPrivateUsage=false

import json
from pathlib import Path

import pytest
from kernel.events import EventType, KernelEvent, KernelStatus
from kernel.protocol import KernelConfig
from kernel_opencode import OpenCodeKernel

# ---------------------------------------------------------------------------
# Sample JSONL lines from a real `opencode run --format json` run
# ---------------------------------------------------------------------------

SIMPLE_HELLO_LINES: list[str] = [
    '{"type":"step_start","timestamp":1776306053049,"sessionID":"ses_26be8ad4affekD3S1MeRQA9FXQ","part":{"id":"prt_d94175fb3001n7XICrtT3kyepu","messageID":"msg_d941753cc001hZZQoaRShc63AV","sessionID":"ses_26be8ad4affekD3S1MeRQA9FXQ","snapshot":"0b17fe9ce95dc59970d2a02d17923e83488b7760","type":"step-start"}}',
    '{"type":"text","timestamp":1776306053101,"sessionID":"ses_26be8ad4affekD3S1MeRQA9FXQ","part":{"id":"prt_d94175fba001bOiq56I3CV3OnR","messageID":"msg_d941753cc001hZZQoaRShc63AV","sessionID":"ses_26be8ad4affekD3S1MeRQA9FXQ","type":"text","text":"Hello","time":{"start":1776306053050,"end":1776306053100}}}',
    '{"type":"step_finish","timestamp":1776306053166,"sessionID":"ses_26be8ad4affekD3S1MeRQA9FXQ","part":{"id":"prt_d94175fee001OdUrAjBvIyjIqb","reason":"stop","snapshot":"0b17fe9ce95dc59970d2a02d17923e83488b7760","messageID":"msg_d941753cc001hZZQoaRShc63AV","sessionID":"ses_26be8ad4affekD3S1MeRQA9FXQ","type":"step-finish","tokens":{"total":20934,"input":20909,"output":1,"reasoning":24,"cache":{"write":0,"read":0}},"cost":0}}',
]

TOOL_USE_LINES: list[str] = [
    '{"type":"step_start","timestamp":1776306064342,"sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","part":{"id":"prt_d94178bd000193NUibRjU5oCi6","messageID":"msg_d9417778f00184WYBNeroz6CRc","sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","snapshot":"0b17fe9ce95dc59970d2a02d17923e83488b7760","type":"step-start"}}',
    '{"type":"tool_use","timestamp":1776306064392,"sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","part":{"type":"tool","tool":"bash","callID":"chatcmpl-tool-a10128c26afd4480","state":{"status":"completed","input":{"command":"ls","description":"List files in current directory"},"output":"AGENTS.md\\nCLAUDE.md\\nREADME.md\\n","metadata":{"output":"AGENTS.md\\nCLAUDE.md\\nREADME.md\\n","exit":0,"description":"List files in current directory","truncated":false},"title":"List files in current directory","time":{"start":1776306064390,"end":1776306064392}},"id":"prt_d94178bd7001vpk7nr1Jy5YGFP","sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","messageID":"msg_d9417778f00184WYBNeroz6CRc"}}',
    '{"type":"step_finish","timestamp":1776306064472,"sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","part":{"id":"prt_d94178c0f0011FqvBhnZuH2UX0","reason":"tool-calls","snapshot":"0b17fe9ce95dc59970d2a02d17923e83488b7760","messageID":"msg_d9417778f00184WYBNeroz6CRc","sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","type":"step-finish","tokens":{"total":20976,"input":20914,"output":36,"reasoning":26,"cache":{"write":0,"read":0}},"cost":0}}',
    '{"type":"step_start","timestamp":1776306070042,"sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","part":{"id":"prt_d9417a218001TLmdFNgcuupxYO","messageID":"msg_d94178c84001T0K167REeXTE0B","sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","snapshot":"0b17fe9ce95dc59970d2a02d17923e83488b7760","type":"step-start"}}',
    '{"type":"text","timestamp":1776306070115,"sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","part":{"id":"prt_d9417a21b001vMUxLv7egFYPXR","messageID":"msg_d94178c84001T0K167REeXTE0B","sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","type":"text","text":"Here are the files.","time":{"start":1776306070043,"end":1776306070114}}}',
    '{"type":"step_finish","timestamp":1776306070173,"sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","part":{"id":"prt_d9417a2630012T2pSAS3DAmi0V","reason":"stop","snapshot":"0b17fe9ce95dc59970d2a02d17923e83488b7760","messageID":"msg_d94178c84001T0K167REeXTE0B","sessionID":"ses_26be889a6ffegJywcGJ9ZXrQMn","type":"step-finish","tokens":{"total":21336,"input":21016,"output":0,"reasoning":320,"cache":{"write":0,"read":0}},"cost":0}}',
]

REASONING_LINES: list[str] = [
    '{"type":"step_start","timestamp":1776306079385,"sessionID":"ses_26be846a2ffeD5yOFDiQljdeth","part":{"id":"prt_d9417c6960019X8XISe722KHqW","messageID":"msg_d9417ba6400142Xbw2iuvbnCMm","sessionID":"ses_26be846a2ffeD5yOFDiQljdeth","snapshot":"0b17fe9ce95dc59970d2a02d17923e83488b7760","type":"step-start"}}',
    '{"type":"reasoning","timestamp":1776306079387,"sessionID":"ses_26be846a2ffeD5yOFDiQljdeth","part":{"id":"prt_d9417c698001L5nXj0eSpwbpu6","messageID":"msg_d9417ba6400142Xbw2iuvbnCMm","sessionID":"ses_26be846a2ffeD5yOFDiQljdeth","type":"reasoning","text":"thinking about it","time":{"start":1776306079384,"end":1776306079385}}}',
    '{"type":"text","timestamp":1776306079401,"sessionID":"ses_26be846a2ffeD5yOFDiQljdeth","part":{"id":"prt_d9417c69a0013OEoFK5ZzGyhf4","messageID":"msg_d9417ba6400142Xbw2iuvbnCMm","sessionID":"ses_26be846a2ffeD5yOFDiQljdeth","type":"text","text":"The answer is 4.","time":{"start":1776306079386,"end":1776306079400}}}',
    '{"type":"step_finish","timestamp":1776306079485,"sessionID":"ses_26be846a2ffeD5yOFDiQljdeth","part":{"id":"prt_d9417c6aa001YtJWe4dBPLKOGc","reason":"stop","snapshot":"0b17fe9ce95dc59970d2a02d17923e83488b7760","messageID":"msg_d9417ba6400142Xbw2iuvbnCMm","sessionID":"ses_26be846a2ffeD5yOFDiQljdeth","type":"step-finish","tokens":{"total":20920,"input":20913,"output":1,"reasoning":6,"cache":{"write":0,"read":0}},"cost":0}}',
]


async def _drain(kernel: OpenCodeKernel) -> list[KernelEvent]:
    """Drain all queued events without blocking forever."""
    events: list[KernelEvent] = []
    while not kernel._queue.empty():
        evt = kernel._queue.get_nowait()
        if evt is not None:
            events.append(evt)
    return events


class TestOpenCodeMapping:
    """Verify _map_event correctly translates OpenCode CLI JSON → kernel events."""

    @pytest.fixture
    def kernel(self) -> OpenCodeKernel:
        k = OpenCodeKernel()
        k._session_id = "test-session"
        return k

    @pytest.mark.asyncio
    async def test_step_start_produces_no_events(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        obj = json.loads(SIMPLE_HELLO_LINES[0])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_text_produces_text_delta(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        obj = json.loads(SIMPLE_HELLO_LINES[1])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.TEXT_DELTA
        assert events[0].content == "Hello"

    @pytest.mark.asyncio
    async def test_step_finish_stop_produces_idle_status(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        obj = json.loads(SIMPLE_HELLO_LINES[2])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.STATUS
        assert events[0].status == KernelStatus.IDLE

    @pytest.mark.asyncio
    async def test_step_finish_tool_calls_produces_no_idle(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        obj = json.loads(TOOL_USE_LINES[2])  # reason: "tool-calls"
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_tool_use_produces_tool_call_and_result(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        obj = json.loads(TOOL_USE_LINES[1])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 2
        assert events[0].type == EventType.TOOL_CALL
        assert events[0].tool == "bash"
        assert events[0].input == {
            "command": "ls",
            "description": "List files in current directory",
        }
        assert events[1].type == EventType.TOOL_RESULT
        assert events[1].tool == "bash"
        assert "AGENTS.md" in (events[1].output or "")

    @pytest.mark.asyncio
    async def test_reasoning_produces_reasoning_delta(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        obj = json.loads(REASONING_LINES[1])
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.REASONING_DELTA
        assert events[0].content == "thinking about it"

    @pytest.mark.asyncio
    async def test_session_id_captured_from_events(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        obj = json.loads(SIMPLE_HELLO_LINES[0])
        await kernel._map_event(obj)

        assert kernel._session_id == "ses_26be8ad4affekD3S1MeRQA9FXQ"

    @pytest.mark.asyncio
    async def test_full_simple_conversation(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        for line in SIMPLE_HELLO_LINES:
            await kernel._map_event(json.loads(line))

        events = await _drain(kernel)
        types = [e.type for e in events]
        assert EventType.TEXT_DELTA in types
        assert EventType.STATUS in types

        text_events = [e for e in events if e.type == EventType.TEXT_DELTA]
        assert text_events[0].content == "Hello"

    @pytest.mark.asyncio
    async def test_full_tool_conversation(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        for line in TOOL_USE_LINES:
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
        kernel: OpenCodeKernel,
    ) -> None:
        cmd = kernel._build_command("hello world")
        assert cmd[0] == "opencode"
        assert cmd[1] == "run"
        assert "hello world" in cmd
        assert "--format" in cmd
        assert "json" in cmd
        assert "--dangerously-skip-permissions" in cmd

    @pytest.mark.asyncio
    async def test_build_command_with_session(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        kernel._config = KernelConfig(session_id="ses_abc123")
        cmd = kernel._build_command("continue")
        assert "--session" in cmd
        assert "ses_abc123" in cmd

    @pytest.mark.asyncio
    async def test_build_command_with_model(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        kernel._config = KernelConfig(
            env={"OPENCODE_MODEL": "anthropic/claude-sonnet-4-20250514"},
        )
        cmd = kernel._build_command("test")
        assert "--model" in cmd
        idx = cmd.index("--model")
        assert cmd[idx + 1] == "anthropic/claude-sonnet-4-20250514"

    @pytest.mark.asyncio
    async def test_build_command_with_variant(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        kernel._config = KernelConfig(env={"OPENCODE_VARIANT": "high"})
        cmd = kernel._build_command("test")
        assert "--variant" in cmd
        idx = cmd.index("--variant")
        assert cmd[idx + 1] == "high"

    def test_write_provider_config_uses_connection_env(
        self,
        kernel: OpenCodeKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setenv("HOME", str(tmp_path))
        kernel._config = KernelConfig(
            env={
                "CONNECTION_URL": "https://connection.test/v1",
                "CONNECTION_API_KEY": "from-connection",
                "KERNEL_OPENCODE_BASE_URL": "https://legacy.test/v1",
                "KERNEL_OPENCODE_API_KEY": "from-legacy",
                "KERNEL_OPENCODE_MODEL_NAME": "model-a",
            },
        )

        kernel._write_provider_config()

        config_path = tmp_path / ".config" / "opencode" / "opencode.json"
        config = json.loads(config_path.read_text())
        options = config["provider"]["customprovider"]["options"]
        assert options["baseURL"] == "https://connection.test/v1"
        assert options["apiKey"] == "from-connection"
        assert config["model"] == "customprovider/model-a"

    def test_write_provider_config_accepts_legacy_opencode_env(
        self,
        kernel: OpenCodeKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setenv("HOME", str(tmp_path))
        kernel._config = KernelConfig(
            env={
                "KERNEL_OPENCODE_BASE_URL": "https://legacy.test/v1",
                "KERNEL_OPENCODE_API_KEY": "from-legacy",
                "KERNEL_OPENCODE_MODEL_NAME": "model-a",
            },
        )

        kernel._write_provider_config()

        config_path = tmp_path / ".config" / "opencode" / "opencode.json"
        config = json.loads(config_path.read_text())
        options = config["provider"]["customprovider"]["options"]
        assert options["baseURL"] == "https://legacy.test/v1"
        assert options["apiKey"] == "from-legacy"
        assert config["model"] == "customprovider/model-a"

    def test_write_provider_config_reports_missing_connection_env(
        self,
        kernel: OpenCodeKernel,
    ) -> None:
        kernel._config = KernelConfig(env={"KERNEL_OPENCODE_MODEL_NAME": "model-a"})

        with pytest.raises(
            ValueError,
            match="missing required environment",
        ) as exc_info:
            kernel._write_provider_config()

        message = str(exc_info.value)
        assert "CONNECTION_URL" in message
        assert "CONNECTION_API_KEY" in message
