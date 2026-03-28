"""Tests for CopilotKernel event mapping.

Uses the real Copilot CLI JSON output format to verify that
``_map_event`` produces the correct standardized kernel events.
"""

import asyncio
import json

import pytest
from kernel.events import EventType, KernelEvent, KernelStatus
from kernel.protocol import KernelConfig
from kernel_copilot import CopilotKernel

# ---------------------------------------------------------------------------
# Sample JSONL lines from a real `copilot -p ... --output-format json` run
# ---------------------------------------------------------------------------

SAMPLE_LINES: list[str] = [
    '{"type":"session.mcp_server_status_changed","data":{"serverName":"github-mcp-server","status":"connected"},"id":"155e1875-93f6-4fa8-bffa-541968c5a551","timestamp":"2026-03-28T20:34:42.437Z","parentId":"fe2b6cc4-644d-4605-9924-ab3287ed6dee","ephemeral":true}',
    '{"type":"session.mcp_servers_loaded","data":{"servers":[{"name":"github-mcp-server","status":"connected","source":"builtin"}]},"id":"4828ea1e-df59-40bc-ab79-f05e742a1636","timestamp":"2026-03-28T20:34:42.597Z","parentId":"46ee0787-9b5d-4b99-a92f-5e640372354d","ephemeral":true}',
    '{"type":"session.tools_updated","data":{"model":"claude-opus-4.6-1m"},"id":"9a38a1e8-58c1-4d98-9772-ab1b7044b06e","timestamp":"2026-03-28T20:34:44.655Z","parentId":"46ee0787-9b5d-4b99-a92f-5e640372354d","ephemeral":true}',
    '{"type":"user.message","data":{"content":"can-you-see-this","transformedContent":"...","attachments":[],"interactionId":"ed0aca40-aeef-4c46-8260-db700ee73c0c"},"id":"4bf8f0e6-a12c-484d-aefe-def513a4df8f","timestamp":"2026-03-28T20:34:44.657Z","parentId":"46ee0787-9b5d-4b99-a92f-5e640372354d"}',
    '{"type":"assistant.turn_start","data":{"turnId":"0","interactionId":"ed0aca40-aeef-4c46-8260-db700ee73c0c"},"id":"e569c416-678c-477a-8a0d-38369acecfe5","timestamp":"2026-03-28T20:34:44.668Z","parentId":"4bf8f0e6-a12c-484d-aefe-def513a4df8f"}',
    '{"type":"assistant.message_delta","data":{"messageId":"c5916a89-2bfa-4b56-aad1-f47857b8af03","deltaContent":"Yes"},"id":"bfee9ded-cdd7-4d59-927b-1d87c04b8474","timestamp":"2026-03-28T20:34:46.815Z","parentId":"e569c416-678c-477a-8a0d-38369acecfe5","ephemeral":true}',
    '{"type":"assistant.message_delta","data":{"messageId":"c5916a89-2bfa-4b56-aad1-f47857b8af03","deltaContent":", I can see your"},"id":"f5e6f560-95a9-4be0-928a-5b2417e885f5","timestamp":"2026-03-28T20:34:46.815Z","parentId":"e569c416-678c-477a-8a0d-38369acecfe5","ephemeral":true}',
    '{"type":"assistant.message_delta","data":{"messageId":"c5916a89-2bfa-4b56-aad1-f47857b8af03","deltaContent":" message!"},"id":"42481462-da83-4ae8-862a-07515c65298d","timestamp":"2026-03-28T20:34:46.816Z","parentId":"e569c416-678c-477a-8a0d-38369acecfe5","ephemeral":true}',
    '{"type":"assistant.message","data":{"messageId":"c5916a89-2bfa-4b56-aad1-f47857b8af03","content":"Yes, I can see your message!","toolRequests":[],"interactionId":"ed0aca40-aeef-4c46-8260-db700ee73c0c","outputTokens":23},"id":"6c6f4d9f-346a-4fbe-acb3-2d2ebbfeea33","timestamp":"2026-03-28T20:34:46.821Z","parentId":"e569c416-678c-477a-8a0d-38369acecfe5"}',
    '{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"c9494b6e-03bf-4089-beb7-3e9ee687c4d2","timestamp":"2026-03-28T20:34:46.822Z","parentId":"6c6f4d9f-346a-4fbe-acb3-2d2ebbfeea33"}',
    '{"type":"result","timestamp":"2026-03-28T20:34:46.825Z","sessionId":"a33fcb66-76b2-4d89-b5ca-2ad99167348b","exitCode":0,"usage":{"premiumRequests":6,"totalApiDurationMs":1641,"sessionDurationMs":5704,"codeChanges":{"linesAdded":0,"linesRemoved":0,"filesModified":[]}}}',
]


def _parse_lines() -> list[dict[str, object]]:
    return [json.loads(line) for line in SAMPLE_LINES]


async def _drain(kernel: CopilotKernel) -> list[KernelEvent]:
    """Drain all queued events without blocking forever."""
    events: list[KernelEvent] = []
    while not kernel._queue.empty():
        evt = kernel._queue.get_nowait()
        if evt is not None:
            events.append(evt)
    return events


class TestCopilotMapping:
    """Verify _map_event correctly translates Copilot CLI JSON → kernel events."""

    @pytest.fixture
    def kernel(self) -> CopilotKernel:
        k = CopilotKernel()
        k._session_id = "test-session"
        return k

    @pytest.mark.asyncio
    async def test_message_delta_produces_text_delta(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[5])  # first message_delta
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.TEXT_DELTA
        assert events[0].content == "Yes"

    @pytest.mark.asyncio
    async def test_multiple_deltas_stream_correctly(
        self,
        kernel: CopilotKernel,
    ) -> None:
        for line in SAMPLE_LINES[5:8]:  # three message_delta lines
            await kernel._map_event(json.loads(line))

        events = await _drain(kernel)
        assert len(events) == 3
        assert all(e.type == EventType.TEXT_DELTA for e in events)
        text = "".join(e.content for e in events if e.content)
        assert text == "Yes, I can see your message!"

    @pytest.mark.asyncio
    async def test_turn_end_produces_idle_status(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[9])  # assistant.turn_end
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.STATUS
        assert events[0].status == KernelStatus.IDLE

    @pytest.mark.asyncio
    async def test_turn_start_produces_no_events(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[4])  # assistant.turn_start
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_session_events_produce_no_events(
        self,
        kernel: CopilotKernel,
    ) -> None:
        for line in SAMPLE_LINES[0:3]:  # session.* events
            await kernel._map_event(json.loads(line))

        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_user_message_produces_no_events(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[3])  # user.message
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_assistant_message_no_tools_produces_no_events(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[8])  # assistant.message (empty toolRequests)
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_assistant_message_with_tool_requests(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj: dict[str, object] = {
            "type": "assistant.message",
            "data": {
                "messageId": "msg-1",
                "content": "",
                "toolRequests": [
                    {"name": "shell", "input": {"cmd": "ls"}},
                    {"name": "read_file", "input": {"path": "/tmp/x"}},
                ],
                "outputTokens": 10,
            },
            "id": "id-1",
            "timestamp": "2026-03-28T20:35:00Z",
            "parentId": "p-1",
        }
        await kernel._map_event(obj)

        events = await _drain(kernel)
        assert len(events) == 2
        assert events[0].type == EventType.TOOL_CALL
        assert events[0].tool == "shell"
        assert events[0].input == {"cmd": "ls"}
        assert events[1].type == EventType.TOOL_CALL
        assert events[1].tool == "read_file"
        assert events[1].input == {"path": "/tmp/x"}

    @pytest.mark.asyncio
    async def test_result_captures_session_id(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj = json.loads(SAMPLE_LINES[10])  # result
        await kernel._map_event(obj)

        assert kernel._session_id == "a33fcb66-76b2-4d89-b5ca-2ad99167348b"
        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_full_stream_produces_expected_sequence(
        self,
        kernel: CopilotKernel,
    ) -> None:
        """Feed all sample lines and verify the overall event sequence."""
        for obj in _parse_lines():
            await kernel._map_event(obj)

        events = await _drain(kernel)
        types = [e.type for e in events]

        # Should have: 3 text_deltas + 1 status(idle)
        assert types.count(EventType.TEXT_DELTA) == 3
        assert types.count(EventType.STATUS) == 1

        # text_deltas come before the status(idle)
        first_delta = types.index(EventType.TEXT_DELTA)
        idle_idx = types.index(EventType.STATUS)
        assert first_delta < idle_idx

        # Verify streamed text
        text = "".join(
            e.content for e in events if e.type == EventType.TEXT_DELTA and e.content
        )
        assert text == "Yes, I can see your message!"

    @pytest.mark.asyncio
    async def test_empty_delta_content_skipped(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj: dict[str, object] = {
            "type": "assistant.message_delta",
            "data": {"messageId": "m1", "deltaContent": ""},
            "id": "id-1",
            "timestamp": "2026-03-28T20:35:00Z",
            "parentId": "p-1",
            "ephemeral": True,
        }
        await kernel._map_event(obj)
        events = await _drain(kernel)
        assert len(events) == 0

    @pytest.mark.asyncio
    async def test_unrecognised_event_produces_no_events(
        self,
        kernel: CopilotKernel,
    ) -> None:
        obj: dict[str, object] = {
            "type": "something.new",
            "data": {},
            "id": "id-1",
            "timestamp": "2026-03-28T20:35:00Z",
            "parentId": "p-1",
        }
        await kernel._map_event(obj)
        events = await _drain(kernel)
        assert len(events) == 0


class TestCopilotKernelLifecycle:
    """Test the kernel's start / name / status without spawning a real process."""

    def test_name(self) -> None:
        assert CopilotKernel().name == "copilot-cli"

    def test_initial_status(self) -> None:
        assert CopilotKernel().status == KernelStatus.IDLE

    @pytest.mark.asyncio
    async def test_start_emits_session_start(self) -> None:
        kernel = CopilotKernel()
        await kernel.start(KernelConfig())

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.SESSION_START
        assert events[0].kernel == "copilot-cli"
        assert events[0].session_id is not None
