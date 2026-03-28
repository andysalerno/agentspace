"""BaseKernel — shared subprocess machinery for harness implementations.

Subclasses implement three methods:
  - harness_cmd(config)  -> the CLI command + args to spawn
  - harness_env(config)  -> extra env vars for the process
  - parse_harness_output(line) -> parse one stdout/stderr line into events
"""

from __future__ import annotations

import asyncio
import os
import uuid
from abc import ABC, abstractmethod
from typing import TYPE_CHECKING

from kernel.events import (
    KernelEvent,
    KernelStatus,
    error,
    session_end,
    session_start,
    status_event,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from kernel.protocol import KernelConfig


class BaseKernel(ABC):
    def __init__(self) -> None:
        self._status = KernelStatus.IDLE
        self._session_id: str = ""
        self._process: asyncio.subprocess.Process | None = None
        self._tasks: list[asyncio.Task[None]] = []
        self._queue: asyncio.Queue[KernelEvent | None] = asyncio.Queue()

    @property
    @abstractmethod
    def name(self) -> str: ...

    @property
    def status(self) -> KernelStatus:
        return self._status

    @abstractmethod
    def harness_cmd(self, config: KernelConfig) -> list[str]:
        """Return the command + args to spawn the harness process."""
        ...

    @abstractmethod
    def harness_env(self, config: KernelConfig) -> dict[str, str]:
        """Return extra environment variables for the harness process."""
        ...

    @abstractmethod
    def parse_harness_output(self, line: str) -> list[KernelEvent]:
        """Parse one line of harness stdout into zero or more kernel events."""
        ...

    async def start(self, config: KernelConfig) -> None:
        self._session_id = uuid.uuid4().hex[:12]
        self._status = KernelStatus.IDLE

        cmd = self.harness_cmd(config)
        env = self.harness_env(config)
        full_env = {**os.environ, **env}

        self._process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=full_env,
        )

        await self._queue.put(session_start(self._session_id, self.name))

        self._tasks = [
            asyncio.create_task(
                self._read_stream(self._process.stdout, is_stderr=False),
            ),
            asyncio.create_task(
                self._read_stream(self._process.stderr, is_stderr=True),
            ),
            asyncio.create_task(self._wait_for_exit()),
        ]

    async def send(self, message: str) -> None:
        if self._process is None or self._process.stdin is None:
            # For harnesses that take input via CLI args rather than stdin,
            # this is a no-op. Subclasses can override.
            return
        self._process.stdin.write((message + "\n").encode())
        await self._process.stdin.drain()

    async def recv(self) -> AsyncIterator[KernelEvent]:
        while True:
            event = await self._queue.get()
            if event is None:
                return
            yield event

    async def stop(self) -> None:
        if self._process is not None and self._process.returncode is None:
            self._process.terminate()
            try:
                await asyncio.wait_for(self._process.wait(), timeout=5.0)
            except TimeoutError:
                self._process.kill()
        self._status = KernelStatus.DONE

    async def _read_stream(
        self,
        stream: asyncio.StreamReader | None,
        *,
        is_stderr: bool,
    ) -> None:
        if stream is None:
            return
        async for raw_line in stream:
            line = raw_line.decode().rstrip("\n").rstrip("\r")
            if not line:
                continue
            if is_stderr:
                await self._queue.put(error(line))
            else:
                events = self.parse_harness_output(line)
                for evt in events:
                    await self._queue.put(evt)

    async def _wait_for_exit(self) -> None:
        if self._process is None:
            return
        returncode = await self._process.wait()
        if returncode != 0:
            self._status = KernelStatus.ERROR
            await self._queue.put(
                error(f"harness exited with code {returncode}"),
            )
        self._status = KernelStatus.DONE
        await self._queue.put(status_event(KernelStatus.DONE))
        await self._queue.put(session_end())
        await self._queue.put(None)  # sentinel to stop recv()
