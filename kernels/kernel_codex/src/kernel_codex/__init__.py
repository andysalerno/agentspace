"""Codex CLI kernel — wraps OpenAI `codex` as an agent harness."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import uuid
from dataclasses import replace
from typing import TYPE_CHECKING, Any, cast

from kernel.events import (
    KernelEvent,
    KernelStatus,
    error,
    session_end,
    session_start,
    status_event,
    text_delta,
    tool_call,
    tool_result,
)
from kernel.protocol import KernelConfig

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

logger = logging.getLogger(__name__)

DEFAULT_WORKSPACE_DIR = "/workspace"

# asyncio's default StreamReader limit is 64 KiB; codex JSONL events can
# easily exceed that when a tool result embeds a fetched web page or other
# large payload. Bump generously to avoid mid-stream readline() failures.
_STREAM_BUFFER_LIMIT = 16 * 1024 * 1024


class CodexKernel:
    """Kernel that wraps OpenAI Codex CLI (`codex exec`)."""

    def __init__(self) -> None:
        self._status = KernelStatus.IDLE
        self._session_id = ""
        self._config = KernelConfig()
        self._process: asyncio.subprocess.Process | None = None
        self._output_task: asyncio.Task[None] | None = None
        self._queue: asyncio.Queue[KernelEvent | None] = asyncio.Queue()
        self._raw_lines: list[str] = []

    @property
    def name(self) -> str:
        return "codex"

    @property
    def status(self) -> KernelStatus:
        return self._status

    @property
    def resume_token(self) -> str | None:
        return self._config.session_id or self._session_id or None

    @property
    def raw_logs(self) -> list[str]:
        return list(self._raw_lines)

    @property
    def _workspace_dir(self) -> str:
        return self._config.env.get("CODEX_WORKSPACE_DIR", DEFAULT_WORKSPACE_DIR)

    async def start(self, config: KernelConfig) -> None:
        self._config = config
        self._session_id = config.session_id or uuid.uuid4().hex[:12]
        self._status = KernelStatus.IDLE
        self._raw_lines = []
        await self._queue.put(session_start(self._session_id, self.name))

    async def send(self, message: str) -> None:
        if self._output_task is not None and not self._output_task.done():
            await self._queue.put(
                error("codex kernel is already processing a request"),
            )
            return

        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        cmd = self._build_command(message)
        env = self._build_env()
        cwd = self._workspace_dir

        logger.info("spawning codex subprocess: cmd=%s cwd=%s", cmd, cwd)

        try:
            self._process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env,
                cwd=cwd,
                limit=_STREAM_BUFFER_LIMIT,
            )
        except FileNotFoundError:
            await self._queue.put(error("codex CLI not found; is it installed?"))
            await self._finish(KernelStatus.ERROR)
            return
        except OSError as exc:
            await self._queue.put(error(f"failed to start codex CLI: {exc}"))
            await self._finish(KernelStatus.ERROR)
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
                await self._process.wait()
        if self._output_task is not None:
            await self._output_task
        self._status = KernelStatus.DONE

    def _build_command(self, message: str) -> list[str]:
        resume_session = self._config.session_id or self._config.env.get(
            "CODEX_SESSION_ID",
        )

        if resume_session:
            cmd = [
                "codex",
                "exec",
                "resume",
                resume_session,
                message,
                "--json",
                "--full-auto",
            ]
        else:
            cmd = [
                "codex",
                "exec",
                message,
                "--json",
                "--full-auto",
            ]

        model = self._config.env.get("CODEX_MODEL")
        if model:
            cmd.extend(["--model", model])

        cwd = self._workspace_dir
        cmd.extend(["-C", cwd])

        for path in self._config.additional_paths:
            cmd.extend(["--add-dir", path])

        extra_paths = self._split_paths_env()
        for path in extra_paths:
            cmd.extend(["--add-dir", path])

        extra_args = self._config.env.get("CODEX_EXTRA_ARGS", "")
        for arg in extra_args.splitlines():
            if arg:
                cmd.append(arg)

        return cmd

    def _build_env(self) -> dict[str, str]:
        env = {**os.environ}
        for key in ("OPENAI_API_KEY",):
            value = self._config.env.get(key)
            if value:
                env[key] = value
        return env

    def _split_paths_env(self) -> list[str]:
        raw = self._config.env.get("CODEX_ADDITIONAL_PATHS", "")
        if not raw:
            return []
        return [p for p in raw.split(os.pathsep) if p]

    async def _read_output(self) -> None:
        if self._process is None:
            return

        stdout_task = asyncio.create_task(self._read_stdout())
        stderr_task = asyncio.create_task(self._read_stderr())
        await asyncio.gather(stdout_task, stderr_task)

        returncode = await self._process.wait()
        if returncode != 0:
            await self._queue.put(error(f"codex exited with code {returncode}"))
            await self._finish(KernelStatus.ERROR)
            return

        await self._finish(KernelStatus.DONE)

    async def _finish(self, status: KernelStatus) -> None:
        self._status = status
        if status != KernelStatus.ERROR:
            await self._queue.put(status_event(KernelStatus.DONE))
        else:
            await self._queue.put(status_event(KernelStatus.ERROR))
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
            self._raw_lines.append(line)
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                await self._queue.put(text_delta(line + "\n"))
                continue
            await self._map_event(obj)

    async def _read_stderr(self) -> None:
        if self._process is None or self._process.stderr is None:
            return
        async for raw_line in self._process.stderr:
            line = raw_line.decode().rstrip("\n").rstrip("\r").strip()
            if line:
                self._raw_lines.append(f"[stderr] {line}")
                await self._queue.put(error(line))

    async def _map_event(self, obj: dict[str, object]) -> None:  # noqa: C901
        event_type = obj.get("type", "")

        if event_type == "thread.started":
            thread_id = obj.get("thread_id")
            if isinstance(thread_id, str):
                self._session_id = thread_id
                self._config = replace(self._config, session_id=thread_id)
            return

        if event_type == "turn.started":
            logger.debug("turn started")
            return

        if event_type == "turn.completed":
            self._status = KernelStatus.IDLE
            await self._queue.put(status_event(KernelStatus.IDLE))
            return

        if event_type == "item.started":
            item = obj.get("item")
            if isinstance(item, dict):
                await self._map_item_started(cast("dict[str, object]", item))
            return

        if event_type == "item.completed":
            item = obj.get("item")
            if isinstance(item, dict):
                await self._map_item_completed(cast("dict[str, object]", item))
            return

        if event_type == "item.streaming_delta":
            item = obj.get("item")
            if isinstance(item, dict):
                await self._map_item_delta(cast("dict[str, object]", item))
            return

        logger.debug("unhandled codex event: %s", event_type)

    async def _map_item_started(self, item: dict[str, object]) -> None:
        item_type = item.get("type", "")
        if item_type == "command_execution":
            command = item.get("command", "")
            if isinstance(command, str) and command:
                await self._queue.put(
                    tool_call("shell", {"cmd": command}),
                )

    async def _map_item_completed(self, item: dict[str, object]) -> None:  # noqa: C901, PLR0912
        item_type = item.get("type", "")

        if item_type == "agent_message":
            text = item.get("text", "")
            if isinstance(text, str) and text:
                await self._queue.put(text_delta(text))
            return

        if item_type == "command_execution":
            command = item.get("command", "")
            output = item.get("aggregated_output", "")
            exit_code = item.get("exit_code")
            tool_name = "shell"
            result_parts: list[str] = []
            if isinstance(output, str) and output:
                result_parts.append(output)
            if exit_code is not None:
                result_parts.append(f"[exit_code: {exit_code}]")
            result_str = "\n".join(result_parts) if result_parts else ""
            if isinstance(command, str) and command:
                await self._queue.put(tool_result(tool_name, result_str))
            return

        if item_type == "tool_call":
            name = item.get("name")
            arguments = item.get("arguments")
            if isinstance(name, str):
                tool_input: dict[str, Any] = {}
                if isinstance(arguments, dict):
                    tool_input = cast("dict[str, Any]", arguments)
                elif isinstance(arguments, str):
                    try:
                        parsed = json.loads(arguments)
                        if isinstance(parsed, dict):
                            tool_input = cast("dict[str, Any]", parsed)
                    except json.JSONDecodeError:
                        tool_input = {"raw": arguments}
                await self._queue.put(tool_call(name, tool_input))
            return

        if item_type == "tool_result":
            name = item.get("name", item.get("tool"))
            output = item.get("output", item.get("result", ""))
            if isinstance(name, str):
                out_str = (
                    output
                    if isinstance(output, str)
                    else json.dumps(output, separators=(",", ":"))
                )
                await self._queue.put(tool_result(name, out_str))
            return

        logger.debug("unhandled item type in item.completed: %s", item_type)

    async def _map_item_delta(self, item: dict[str, object]) -> None:
        item_type = item.get("type", "")
        if item_type == "agent_message":
            text = item.get("text", "")
            if isinstance(text, str) and text:
                await self._queue.put(text_delta(text))
