import asyncio
import json

import pytest
from kernel.events import EventType, KernelStatus
from kernel.protocol import KernelConfig

from kernel_echo import EchoKernel


class TestEchoKernel:
    @pytest.fixture
    def kernel(self) -> EchoKernel:
        return EchoKernel()

    def test_name(self, kernel: EchoKernel) -> None:
        assert kernel.name == "echo"

    def test_initial_status(self, kernel: EchoKernel) -> None:
        assert kernel.status == KernelStatus.IDLE

    def _run(self, coro: object) -> object:
        return asyncio.get_event_loop().run_until_complete(coro)  # type: ignore[arg-type]

    @pytest.mark.asyncio
    async def test_echo_message(self, kernel: EchoKernel) -> None:
        config = KernelConfig()
        await kernel.start(config)
        await kernel.send("hello world")

        events = []
        async for event in kernel.recv():
            events.append(event)

        # Should have: session_start, status(busy), text_delta(hello), text_delta( world),
        # status(done), session_end
        types = [e.type for e in events]
        assert types[0] == EventType.SESSION_START
        assert types[1] == EventType.STATUS
        assert EventType.TEXT_DELTA in types

        # Verify session_start has correct kernel name
        assert events[0].kernel == "echo"
        assert events[0].session_id is not None

        # Verify text deltas reconstruct the original message
        text_parts = [e.content for e in events if e.type == EventType.TEXT_DELTA]
        assert "".join(t for t in text_parts if t) == "hello world"

        # Verify JSONL serialization works for all events
        for event in events:
            line = event.to_jsonl()
            parsed = json.loads(line)
            assert "type" in parsed
            assert "ts" in parsed

    @pytest.mark.asyncio
    async def test_status_transitions(self, kernel: EchoKernel) -> None:
        config = KernelConfig()
        await kernel.start(config)
        await kernel.send("test")

        events = []
        async for event in kernel.recv():
            events.append(event)

        status_events = [e for e in events if e.type == EventType.STATUS]
        statuses = [e.status for e in status_events]
        assert KernelStatus.BUSY in statuses
        assert KernelStatus.DONE in statuses

    @pytest.mark.asyncio
    async def test_stop(self, kernel: EchoKernel) -> None:
        config = KernelConfig()
        await kernel.start(config)
        await kernel.stop()
        assert kernel.status == KernelStatus.DONE
