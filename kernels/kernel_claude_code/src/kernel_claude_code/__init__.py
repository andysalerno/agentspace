"""Claude Code kernel — wraps `claude` as an agent harness."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import uuid
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING, Any, cast

from kernel.events import (
    KernelEvent,
    KernelStatus,
    error,
    reasoning_delta,
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
DEFAULT_ALLOWED_TOOLS = "Bash,Read,Edit,Write,Glob,Grep,TodoWrite,Skill,Task"


def _ensure_directory(path: str) -> None:
    Path(path).mkdir(parents=True, exist_ok=True)


class ClaudeCodeKernel:
    """Kernel that wraps Claude Code CLI (`claude`)."""

    def __init__(self) -> None:
        self._status = KernelStatus.IDLE
        self._session_id = ""
        self._resume_session_id: str | None = None
        self._config = KernelConfig()
        self._process: asyncio.subprocess.Process | None = None
        self._output_task: asyncio.Task[None] | None = None
        self._queue: asyncio.Queue[KernelEvent | None] = asyncio.Queue()
        self._raw_lines: list[str] = []
        self._tool_call_names: dict[str, str] = {}

    @property
    def name(self) -> str:
        return "claude-code"

    @property
    def status(self) -> KernelStatus:
        return self._status

    @property
    def resume_token(self) -> str | None:
        return self._resume_session_id or self._config.session_id

    @property
    def raw_logs(self) -> list[str]:
        return list(self._raw_lines)

    @property
    def _workspace_dir(self) -> str:
        return self._config.env.get("CLAUDE_CODE_WORKSPACE_DIR", DEFAULT_WORKSPACE_DIR)

    async def start(self, config: KernelConfig) -> None:
        self._config = config
        self._session_id = config.session_id or uuid.uuid4().hex[:12]
        self._resume_session_id = config.session_id
        self._status = KernelStatus.IDLE
        self._raw_lines = []
        self._tool_call_names = {}
        await self._queue.put(session_start(self._session_id, self.name))

    async def send(self, message: str) -> None:
        if self._output_task is not None and not self._output_task.done():
            await self._queue.put(
                error("claude-code kernel is already processing a request"),
            )
            return

        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        cmd = self._build_command(message)
        env = self._build_env()
        cwd = self._workspace_dir
        await asyncio.to_thread(_ensure_directory, cwd)

        logger.info("spawning claude subprocess: cmd=%s cwd=%s", cmd, cwd)

        try:
            self._process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env,
                cwd=cwd,
            )
        except FileNotFoundError:
            await self._queue.put(error("claude CLI not found; is it installed?"))
            await self._finish(KernelStatus.ERROR)
            return
        except OSError as exc:
            await self._queue.put(error(f"failed to start claude CLI: {exc}"))
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
        cmd = [
            "claude",
            "--print",
            "--bare",
            "--output-format",
            "stream-json",
            "--verbose",
            "--tools",
            self._build_tools_arg(),
            "--allow-dangerously-skip-permissions",
            "--dangerously-skip-permissions",
        ]

        effort = self._config.env.get("CLAUDE_CODE_REASONING_EFFORT")
        if effort:
            cmd.extend(["--effort", effort])

        resume_session = self._config.session_id or self._config.env.get(
            "CLAUDE_CODE_SESSION_ID",
        )
        if resume_session:
            cmd.extend(["--resume", resume_session])

        additional_paths = list(self._config.additional_paths)
        additional_paths.extend(self._split_paths_env())
        for path in additional_paths:
            cmd.extend(["--add-dir", path])

        cmd.extend(self._iter_extra_arg_tokens())
        cmd.append(message)
        return cmd

    def _build_env(self) -> dict[str, str]:
        env = {**os.environ}
        env["WORKSPACE_DIR"] = self._workspace_dir

        for key in (
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "CLAUDE_CODE_REASONING_EFFORT",
            "CLAUDE_CODE_ADDITIONAL_TOOLS",
            "CLAUDE_CODE_ADDITIONAL_PATHS",
            "CLAUDE_CODE_EXTRA_ARGS",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_WORKSPACE_DIR",
        ):
            value = self._config.env.get(key)
            if value:
                env[key] = value

        return env

    def _build_tools_arg(self) -> str:
        additional_tools = self._config.env.get("CLAUDE_CODE_ADDITIONAL_TOOLS", "")
        if not additional_tools:
            return DEFAULT_ALLOWED_TOOLS
        return f"{DEFAULT_ALLOWED_TOOLS},{additional_tools}"

    def _split_paths_env(self) -> list[str]:
        raw = self._config.env.get("CLAUDE_CODE_ADDITIONAL_PATHS", "")
        if not raw:
            return []
        return self._split_paths(raw)

    def _iter_extra_arg_tokens(self) -> list[str]:
        raw = self._config.env.get("CLAUDE_CODE_EXTRA_ARGS", "")
        return [arg for arg in (line.strip() for line in raw.splitlines()) if arg]

    def _split_paths(self, raw: str) -> list[str]:
        parts = [segment for segment in re.split(r"[\n;]+", raw) if segment]
        if len(parts) != 1 or ":" not in raw:
            return parts

        colon_parts = [segment for segment in raw.split(":") if segment]
        if all(segment.startswith("/") for segment in colon_parts):
            return colon_parts
        return parts

    async def _read_output(self) -> None:
        if self._process is None:
            return

        stdout_task = asyncio.create_task(self._read_stdout())
        stderr_task = asyncio.create_task(self._read_stderr())
        await asyncio.gather(stdout_task, stderr_task)

        returncode = await self._process.wait()
        if returncode != 0:
            await self._queue.put(error(f"claude exited with code {returncode}"))
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
            if isinstance(obj, dict):
                await self._map_event(cast("dict[str, object]", obj))
            else:
                await self._queue.put(text_delta(json.dumps(obj) + "\n"))

    async def _read_stderr(self) -> None:
        if self._process is None or self._process.stderr is None:
            return

        async for raw_line in self._process.stderr:
            line = raw_line.decode().rstrip("\n").rstrip("\r").strip()
            if line:
                self._raw_lines.append(f"[stderr] {line}")
                await self._queue.put(error(line))

    async def _map_event(self, obj: dict[str, object]) -> None:  # noqa: C901, PLR0911, PLR0912
        event_type = obj.get("type", "")
        data = self._as_dict(obj.get("data"))
        message = self._as_dict(obj.get("message"))
        delta = self._as_dict(obj.get("delta"))
        self._capture_session_id(obj, data)

        if event_type == "content_block_delta":
            delta_type = delta.get("type")
            if delta_type == "text_delta":
                text = self._first_string(delta.get("text"), delta.get("delta"))
                if text:
                    await self._queue.put(text_delta(text))
            elif delta_type in {"thinking_delta", "reasoning_delta"}:
                text = self._first_string(delta.get("text"), delta.get("thinking"))
                if text:
                    await self._queue.put(reasoning_delta(text))
            return

        if event_type == "content_block_start":
            content_block = self._as_dict(
                data.get("content_block", obj.get("content_block")),
            )
            if content_block:
                await self._emit_content_item(content_block)
            return

        if event_type in {"tool_use", "tool_call", "tool_request"}:
            await self._emit_tool_call(data or obj)
            return

        if event_type in {"tool_result", "tool_response"}:
            await self._emit_tool_result(data or obj)
            return

        if await self._emit_content_items(message.get("content")):
            return
        if await self._emit_content_items(data.get("content")):
            return

        if self._looks_like_reasoning_event(event_type):
            reasoning = self._first_string(
                data.get("thinking"),
                data.get("text"),
                data.get("content"),
                delta.get("text"),
            )
            if reasoning:
                await self._queue.put(reasoning_delta(reasoning))
                return

        direct_text = self._first_string(
            data.get("text"),
            data.get("delta"),
            data.get("content"),
            data.get("message"),
            message.get("text"),
            message.get("content"),
            obj.get("text"),
            obj.get("content"),
        )
        if direct_text and self._looks_like_text_event(event_type):
            await self._queue.put(text_delta(direct_text))
            return

        status = self._first_string(data.get("status"), obj.get("status"))
        if status in {"busy", "in_progress"}:
            self._status = KernelStatus.BUSY
            await self._queue.put(status_event(KernelStatus.BUSY))
            return
        if status in {"completed", "done", "idle"} or event_type in {
            "assistant.turn_end",
            "message_stop",
            "result",
        }:
            self._status = KernelStatus.IDLE
            await self._queue.put(status_event(KernelStatus.IDLE))
            return

        ignored_events = {
            "message_start",
            "message_delta",
            "content_block_stop",
            "ping",
            "system",
        }
        if event_type in ignored_events:
            logger.info("ignoring claude event: %s", event_type)
            return

        logger.info("unhandled claude event: type=%s data=%s", event_type, data)

    async def _emit_content_items(self, content: object) -> bool:
        if not isinstance(content, list):
            return False

        emitted = False
        for item in cast("list[object]", content):
            if not isinstance(item, dict):
                continue
            item_dict = cast("dict[str, object]", item)
            emitted_item = await self._emit_content_item(item_dict)
            emitted = emitted_item or emitted
        return emitted

    async def _emit_content_item(self, item: dict[str, object]) -> bool:  # noqa: PLR0911
        item_type = self._first_string(item.get("type"))
        if item_type == "text":
            text = self._first_string(item.get("text"), item.get("content"))
            if text:
                await self._queue.put(text_delta(text))
                return True
            return False

        if item_type in {"thinking", "reasoning"}:
            content = self._first_string(
                item.get("thinking"),
                item.get("text"),
                item.get("content"),
            )
            if content:
                await self._queue.put(reasoning_delta(content))
                return True
            return False

        if item_type == "tool_use":
            return await self._emit_tool_call(item)

        if item_type == "tool_result":
            return await self._emit_tool_result(item)

        return False

    async def _emit_tool_call(self, payload: dict[str, object]) -> bool:
        tool_name = payload.get("name", payload.get("tool"))
        tool_input = payload.get("input", payload.get("arguments", {}))
        if not isinstance(tool_name, str) or not isinstance(tool_input, dict):
            return False
        tool_use_id = payload.get("id", payload.get("tool_use_id"))
        if isinstance(tool_use_id, str):
            self._tool_call_names[tool_use_id] = tool_name
        await self._queue.put(tool_call(tool_name, cast("dict[str, Any]", tool_input)))
        return True

    async def _emit_tool_result(self, payload: dict[str, object]) -> bool:
        tool_name = payload.get("name", payload.get("tool"))
        if not isinstance(tool_name, str):
            tool_use_id = payload.get("tool_use_id")
            if isinstance(tool_use_id, str):
                tool_name = self._tool_call_names.pop(tool_use_id, None)
        raw_output = payload.get(
            "output",
            payload.get("result", payload.get("content", "")),
        )
        if not isinstance(tool_name, str):
            return False
        output = self._stringify_output(raw_output)
        await self._queue.put(tool_result(tool_name, output))
        return True

    def _capture_session_id(
        self,
        obj: dict[str, object],
        data: dict[str, object],
    ) -> None:
        session_id = self._first_string(
            obj.get("session_id"),
            obj.get("sessionId"),
            data.get("session_id"),
            data.get("sessionId"),
        )
        if session_id is None:
            return
        self._resume_session_id = session_id
        self._config = replace(self._config, session_id=session_id)

    def _stringify_output(self, raw_output: object) -> str:
        if isinstance(raw_output, str):
            return raw_output
        if raw_output is None:
            return ""
        if isinstance(raw_output, list):
            parts: list[str] = []
            for item in cast("list[object]", raw_output):
                if isinstance(item, dict):
                    item_dict = cast("dict[str, object]", item)
                    text = self._first_string(
                        item_dict.get("text"),
                        item_dict.get("content"),
                    )
                    parts.append(text or json.dumps(item_dict, separators=(",", ":")))
                else:
                    parts.append(str(item))
            return "".join(parts)
        return json.dumps(raw_output, separators=(",", ":"))

    def _first_string(self, *values: object) -> str | None:
        for value in values:
            if isinstance(value, str) and value:
                return value
        return None

    def _looks_like_reasoning_event(self, event_type: object) -> bool:
        return isinstance(event_type, str) and (
            "reasoning" in event_type or "thinking" in event_type
        )

    def _looks_like_text_event(self, event_type: object) -> bool:
        return isinstance(event_type, str) and (
            "text" in event_type or "message" in event_type or "assistant" in event_type
        )

    def _as_dict(self, value: object) -> dict[str, object]:
        if isinstance(value, dict):
            return cast("dict[str, object]", value)
        return {}
