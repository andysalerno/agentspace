"""Echo kernel — a trivial kernel for testing that echoes input back."""

from __future__ import annotations

import asyncio
import uuid
from typing import TYPE_CHECKING

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


class EchoKernel:
    """Kernel that echoes the input message back as text_delta events.

    Does not spawn any subprocess — purely in-process.
    """

    def __init__(self) -> None:
        self._status = KernelStatus.IDLE
        self._session_id: str = ""
        self._queue: asyncio.Queue[KernelEvent | None] = asyncio.Queue()

    @property
    def name(self) -> str:
        return "echo"

    @property
    def status(self) -> KernelStatus:
        return self._status

    async def start(self, _config: KernelConfig) -> None:
        self._session_id = uuid.uuid4().hex[:12]
        self._status = KernelStatus.IDLE
        await self._queue.put(session_start(self._session_id, self.name))

    async def send(self, message: str) -> None:
        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        # Echo the message back, word by word to simulate streaming
        words = message.split()
        for i, word in enumerate(words):
            chunk = word if i == 0 else " " + word
            await self._queue.put(text_delta(chunk))
            await asyncio.sleep(0)  # yield to event loop

        self._status = KernelStatus.DONE
        await self._queue.put(status_event(KernelStatus.DONE))
        await self._queue.put(session_end())
        await self._queue.put(None)  # sentinel

    async def recv(self) -> AsyncIterator[KernelEvent]:
        while True:
            event = await self._queue.get()
            if event is None:
                return
            yield event

    async def stop(self) -> None:
        self._status = KernelStatus.DONE
