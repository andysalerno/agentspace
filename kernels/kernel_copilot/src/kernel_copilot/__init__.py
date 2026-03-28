"""Copilot CLI kernel — wraps `copilot` as an agent harness.

This shells out to `copilot -p <prompt>` in non-interactive mode
with `--output-format json` to get native JSONL output, then maps
that to standard kernel events.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import uuid
from typing import TYPE_CHECKING

from kernel.events import (
    KernelEvent,
    KernelStatus,
    error,
    session_end,
    session_start,
    status_event,
    text_delta,
    tool_call,
)
from kernel.protocol import KernelConfig

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

logger = logging.getLogger(__name__)


class CopilotKernel:
    """Kernel that wraps GitHub Copilot CLI (`copilot`).

    Unlike echo kernel, this defers subprocess spawning until send()
    because `copilot` takes the prompt as a CLI argument.

    Uses `copilot -p <prompt> --output-format json` for structured
    JSONL output with `--allow-all-tools` to run non-interactively.
    """

    def __init__(self) -> None:
        self._status = KernelStatus.IDLE
        self._session_id: str = ""
        self._config: KernelConfig = KernelConfig()
        self._process: asyncio.subprocess.Process | None = None
        self._output_task: asyncio.Task[None] | None = None
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
        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        cmd = [
            "copilot",
            "-p",
            message,
            "--output-format",
            "json",
            "--allow-all",
            "--no-ask-user",
            "--no-auto-update",
            "--no-color",
            "-s",
        ]

        model = self._config.env.get("COPILOT_MODEL", "")
        if model:
            cmd.extend(["--model", model])

        env = {**os.environ}
        gh_token = self._config.env.get("GH_TOKEN", "")
        if gh_token:
            env["GH_TOKEN"] = gh_token

        logger.debug("spawning subprocess: %s", cmd)

        try:
            self._process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env,
            )
        except FileNotFoundError:
            await self._queue.put(error("copilot CLI not found — is it installed?"))
            self._status = KernelStatus.ERROR
            await self._queue.put(status_event(KernelStatus.DONE))
            await self._queue.put(session_end())
            await self._queue.put(None)
            return

        self._output_task = asyncio.create_task(self._read_output())

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

        await self._read_stdout()
        await self._read_stderr()

        returncode = await self._process.wait()
        if returncode != 0:
            self._status = KernelStatus.ERROR
            await self._queue.put(error(f"copilot exited with code {returncode}"))

        self._status = KernelStatus.DONE
        await self._queue.put(status_event(KernelStatus.DONE))
        await self._queue.put(session_end())
        await self._queue.put(None)

    async def _read_stdout(self) -> None:
        if self._process is None or self._process.stdout is None:
            return
        async for raw_line in self._process.stdout:
            line = raw_line.decode().rstrip("\n").rstrip("\r").strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                await self._queue.put(text_delta(line + "\n"))
                continue
            await self._map_event(obj)

    async def _read_stderr(self) -> None:
        if self._process is None or self._process.stderr is None:
            return
        remaining = await self._process.stderr.read()
        if remaining:
            for raw in remaining.decode().splitlines():
                stripped = raw.strip()
                if stripped:
                    await self._queue.put(error(stripped))

    async def _map_event(self, obj: dict[str, object]) -> None:
        """Map a Copilot CLI JSONL object to kernel events.

        The Copilot CLI with ``--output-format json`` emits events shaped like::

            {"type": "<namespace>.<event>", "data": {...}, "id": "...",
             "timestamp": "...", "parentId": "...", "ephemeral": true|false}

        The top-level ``result`` event is an exception — it has ``sessionId``,
        ``exitCode``, and ``usage`` at the top level (no ``data`` wrapper).
        """
        event_type = obj.get("type", "")
        data = obj.get("data")
        if not isinstance(data, dict):
            data = {}

        # --- streaming text chunks ---
        if event_type == "assistant.message_delta":
            content = data.get("deltaContent", "")
            if isinstance(content, str) and content:
                await self._queue.put(text_delta(content))

        # --- turn lifecycle ---
        elif event_type == "assistant.turn_start":
            logger.debug("turn started: turnId=%s", data.get("turnId"))

        elif event_type == "assistant.turn_end":
            self._status = KernelStatus.IDLE
            await self._queue.put(status_event(KernelStatus.IDLE))

        # --- full message (content already streamed via deltas) ---
        elif event_type == "assistant.message":
            logger.debug(
                "message complete: messageId=%s tokens=%s",
                data.get("messageId"),
                data.get("outputTokens"),
            )
            # Emit tool_call events for any tool requests attached to
            # the completed message.
            tool_requests = data.get("toolRequests", [])
            if isinstance(tool_requests, list):
                for req in tool_requests:
                    if not isinstance(req, dict):
                        continue
                    t_name = req.get("name", req.get("tool", "unknown"))
                    t_input = req.get("input", {})
                    if isinstance(t_name, str) and isinstance(t_input, dict):
                        await self._queue.put(tool_call(t_name, t_input))

        # --- session-level result (different shape — no `data` wrapper) ---
        elif event_type == "result":
            session_id = obj.get("sessionId")
            if isinstance(session_id, str):
                self._session_id = session_id
            exit_code = obj.get("exitCode")
            logger.debug(
                "result: exitCode=%s sessionId=%s",
                exit_code,
                session_id,
            )

        # --- user message echo ---
        elif event_type == "user.message":
            logger.debug("user message echoed")

        # --- session infrastructure (ephemeral) ---
        elif event_type in (
            "session.mcp_server_status_changed",
            "session.mcp_servers_loaded",
            "session.tools_updated",
        ):
            logger.debug("session infra event: %s", event_type)

        else:
            logger.debug("unhandled copilot event: %s", event_type)
