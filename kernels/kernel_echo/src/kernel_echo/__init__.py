"""Echo kernel — a trivial kernel for testing that echoes input back."""

from __future__ import annotations

import asyncio
import os
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
        self._delay_seconds = float(os.environ.get("KERNEL_ECHO_DELAY_SECONDS", "0.02"))
        self._emit_task: asyncio.Task[None] | None = None

    @property
    def name(self) -> str:
        return "echo"

    @property
    def status(self) -> KernelStatus:
        return self._status

    @property
    def resume_token(self) -> str | None:
        return self._session_id or None

    async def start(self, _config: KernelConfig) -> None:
        self._session_id = uuid.uuid4().hex[:12]
        self._status = KernelStatus.IDLE
        await self._queue.put(session_start(self._session_id, self.name))

    async def send(self, message: str) -> None:
        if self._emit_task is not None and not self._emit_task.done():
            return
        self._emit_task = asyncio.create_task(self._emit_message(message))

    async def _emit_message(self, message: str) -> None:
        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        # Echo the message back, word by word to simulate streaming
        words = message.split()
        for i, word in enumerate(words):
            chunk = word if i == 0 else " " + word
            await self._queue.put(text_delta(chunk))
            await asyncio.sleep(self._delay_seconds)

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
        if self._emit_task is not None:
            await self._emit_task
        self._status = KernelStatus.DONE
