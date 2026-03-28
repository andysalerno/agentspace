"""Copilot CLI kernel — wraps `gh copilot` as an agent harness.

This shells out to `gh copilot suggest` (or `gh copilot explain`)
and maps the output to standard kernel events.

NOTE: The exact invocation will be refined. This is a best-effort
implementation based on known `gh copilot` behavior.
"""

from __future__ import annotations

import asyncio
import re
import uuid
from collections.abc import AsyncIterator

from kernel.events import (
    KernelEvent,
    KernelStatus,
    error,
    session_end,
    session_start,
    status_event,
    text_delta,
)
from kernel.protocol import KernelConfig

# Regex to strip ANSI escape codes from terminal output
_ANSI_RE = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")


class CopilotKernel:
    """Kernel that wraps GitHub Copilot CLI (`gh copilot`).

    Unlike echo kernel, this defers subprocess spawning until send()
    because `gh copilot` takes the prompt as a CLI argument.
    """

    def __init__(self) -> None:
        self._status = KernelStatus.IDLE
        self._session_id: str = ""
        self._config: KernelConfig = KernelConfig()
        self._process: asyncio.subprocess.Process | None = None
        self._queue: asyncio.Queue[KernelEvent | None] = asyncio.Queue()

    @property
    def name(self) -> str:
        return "copilot-cli"

    @property
    def status(self) -> KernelStatus:
        return self._status

    async def start(self, config: KernelConfig) -> None:
        self._session_id = uuid.uuid4().hex[:12]
        self._config = config
        self._status = KernelStatus.IDLE
        await self._queue.put(session_start(self._session_id, self.name))

    async def send(self, message: str) -> None:
        import os

        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        cmd = [
            "gh",
            "copilot",
            "suggest",
            "-t",
            "shell",
            message,
        ]

        env = {**os.environ}
        gh_token = self._config.env.get("GH_TOKEN", "")
        if gh_token:
            env["GH_TOKEN"] = gh_token
        env["GH_PROMPT_DISABLED"] = "1"
        env["NO_COLOR"] = "1"

        try:
            self._process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env,
            )
        except FileNotFoundError:
            await self._queue.put(error("gh CLI not found — is it installed?"))
            self._status = KernelStatus.ERROR
            await self._queue.put(status_event(KernelStatus.DONE))
            await self._queue.put(session_end())
            await self._queue.put(None)
            return

        asyncio.create_task(self._read_output())

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

    async def _read_output(self) -> None:
        if self._process is None:
            return

        # Read stdout
        if self._process.stdout is not None:
            async for raw_line in self._process.stdout:
                line = raw_line.decode().rstrip("\n").rstrip("\r")
                clean = _ANSI_RE.sub("", line).strip()
                if clean:
                    await self._queue.put(text_delta(clean + "\n"))

        # Read any stderr
        if self._process.stderr is not None:
            remaining = await self._process.stderr.read()
            if remaining:
                for line in remaining.decode().splitlines():
                    line = line.strip()
                    if line:
                        await self._queue.put(error(line))

        returncode = await self._process.wait()
        if returncode != 0:
            self._status = KernelStatus.ERROR
            await self._queue.put(error(f"gh copilot exited with code {returncode}"))

        self._status = KernelStatus.DONE
        await self._queue.put(status_event(KernelStatus.DONE))
        await self._queue.put(session_end())
        await self._queue.put(None)
