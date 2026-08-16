"""Copilot CLI kernel — wraps `copilot` as an agent harness."""

from __future__ import annotations

import asyncio
import json
import logging
import uuid
from dataclasses import replace
from pathlib import Path
from typing import TYPE_CHECKING, Any, cast

from copilot_launch import (
    CopilotLaunchConfig,
    build_chat_argv,
    build_chat_launch,
    build_copilot_environment,
)
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

# asyncio's default StreamReader limit is 64 KiB; copilot JSONL events can
# easily exceed that when a tool result embeds a fetched web page or other
# large payload. Bump generously to avoid mid-stream readline() failures.
_STREAM_BUFFER_LIMIT = 16 * 1024 * 1024


def _ensure_directory(path: str) -> None:
    Path(path).mkdir(parents=True, exist_ok=True)


class CopilotKernel:
    """Kernel that wraps GitHub Copilot CLI (`copilot`)."""

    def __init__(self) -> None:
        self._status = KernelStatus.IDLE
        self._session_id = ""
        self._config = KernelConfig()
        self._process: asyncio.subprocess.Process | None = None
        self._output_task: asyncio.Task[None] | None = None
        self._queue: asyncio.Queue[KernelEvent | None] = asyncio.Queue()
        self._raw_lines: list[str] = []
        self._tool_call_names: dict[str, str] = {}

    @property
    def name(self) -> str:
        return "copilot-cli"

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
        """Internal workspace directory — always a clean, kernel-managed path."""
        return self._config.env.get("COPILOT_WORKSPACE_DIR", DEFAULT_WORKSPACE_DIR)

    async def start(self, config: KernelConfig) -> None:
        self._config = config
        self._session_id = (
            config.session_id
            or config.env.get("COPILOT_SESSION_ID")
            or str(uuid.uuid4())
        )
        self._status = KernelStatus.IDLE
        self._raw_lines = []
        await self._queue.put(session_start(self._session_id, self.name))

    async def send(self, message: str) -> None:
        if self._output_task is not None and not self._output_task.done():
            await self._queue.put(
                error("copilot kernel is already processing a request"),
            )
            return

        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        cwd = self._workspace_dir

        try:
            await asyncio.to_thread(_ensure_directory, cwd)
            launch = await asyncio.to_thread(
                build_chat_launch,
                self._launch_config(),
                message,
            )
        except (OSError, ValueError) as exc:
            await self._queue.put(error(f"failed to start copilot CLI: {exc}"))
            await self._finish(KernelStatus.ERROR)
            return

        logger.info(
            "spawning copilot subprocess: cmd=%s cwd=%s",
            list(launch.redacted_argv),
            launch.cwd,
        )

        try:
            self._process = await asyncio.create_subprocess_exec(
                *launch.argv,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=launch.environment,
                cwd=launch.cwd,
                limit=_STREAM_BUFFER_LIMIT,
            )
        except FileNotFoundError:
            await self._queue.put(error("copilot CLI not found; is it installed?"))
            await self._finish(KernelStatus.ERROR)
            return
        except OSError as exc:
            await self._queue.put(error(f"failed to start copilot CLI: {exc}"))
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
        return list(build_chat_argv(self._launch_config(), message))

    def _build_env(self) -> dict[str, str]:
        return build_copilot_environment(self._config.env)

    def _launch_config(self) -> CopilotLaunchConfig:
        return CopilotLaunchConfig(
            session_id=self._session_id
            or self._config.session_id
            or self._config.env.get("COPILOT_SESSION_ID", ""),
            env=self._config.env,
            additional_paths=self._config.additional_paths,
            workspace_dir=self._workspace_dir,
        )

    async def _read_output(self) -> None:
        if self._process is None:
            return

        stdout_task = asyncio.create_task(self._read_stdout())
        stderr_task = asyncio.create_task(self._read_stderr())
        await asyncio.gather(stdout_task, stderr_task)

        returncode = await self._process.wait()
        if returncode != 0:
            await self._queue.put(error(f"copilot exited with code {returncode}"))
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

    async def _map_event(self, obj: dict[str, object]) -> None:  # noqa: C901, PLR0911, PLR0912, PLR0915
        event_type = obj.get("type", "")
        raw_data = obj.get("data")
        data = cast("dict[str, object]", raw_data) if isinstance(raw_data, dict) else {}

        if event_type == "assistant.message_delta":
            content = data.get("deltaContent", "")
            if isinstance(content, str) and content:
                await self._queue.put(text_delta(content))
            return

        if event_type == "assistant.reasoning_delta":
            content = data.get("deltaContent", "")
            if isinstance(content, str) and content:
                await self._queue.put(reasoning_delta(content))
            return

        if event_type == "assistant.turn_start":
            logger.debug("turn started: turnId=%s", data.get("turnId", ""))
            return

        if event_type == "assistant.turn_end":
            self._status = KernelStatus.IDLE
            await self._queue.put(status_event(KernelStatus.IDLE))
            return

        if event_type == "assistant.message":
            logger.debug(
                "message complete: messageId=%s tokens=%s",
                data.get("messageId", ""),
                data.get("outputTokens", ""),
            )
            await self._emit_tool_calls(data.get("toolRequests"))
            return

        if event_type in {"tool.call", "tool_call", "assistant.tool_call"}:
            tool_name, tool_input = self._extract_tool_payload(data)
            if tool_name is not None and tool_input is not None:
                await self._queue.put(tool_call(tool_name, tool_input))
            return

        if event_type in {"tool.result", "tool_result", "assistant.tool_result"}:
            tool_name, tool_output = self._extract_tool_result(data)
            if tool_name is not None and tool_output is not None:
                await self._queue.put(tool_result(tool_name, tool_output))
            return

        if event_type == "tool.execution_start":
            call_id = data.get("toolCallId")
            tool_name = data.get("toolName")
            if isinstance(call_id, str) and isinstance(tool_name, str):
                self._tool_call_names[call_id] = tool_name
            return

        if event_type == "tool.execution_complete":
            call_id = data.get("toolCallId")
            if isinstance(call_id, str):
                tool_name = self._tool_call_names.pop(call_id, None)
            else:
                tool_name = None
            if tool_name is not None:
                raw_result = data.get("result")
                if isinstance(raw_result, dict):
                    result_dict = cast("dict[str, object]", raw_result)
                    output = str(
                        result_dict.get("detailedContent")
                        or result_dict.get("content")
                        or "",
                    )
                else:
                    output = str(raw_result) if raw_result else ""
                await self._queue.put(tool_result(tool_name, output))
            return

        if event_type == "result":
            session_id = obj.get("sessionId")
            if isinstance(session_id, str):
                self._session_id = session_id
                self._config = replace(self._config, session_id=session_id)
            exit_code = obj.get("exitCode", "")
            logger.debug(
                "result: exitCode=%s sessionId=%s",
                exit_code,
                session_id,
            )
            return

        if event_type == "user.message":
            logger.debug("user message echoed")
            return

        if event_type in {
            "session.mcp_server_status_changed",
            "session.mcp_servers_loaded",
            "session.tools_updated",
        }:
            logger.debug("session infra event: %s", event_type)
            return

        logger.debug("unhandled copilot event: %s", event_type)

    async def _emit_tool_calls(self, tool_requests: object) -> None:
        if not isinstance(tool_requests, list):
            return
        requests = cast("list[object]", tool_requests)
        for request in requests:
            if not isinstance(request, dict):
                continue
            tool_name, tool_input = self._extract_tool_payload(
                cast("dict[str, object]", request),
            )
            if tool_name is not None and tool_input is not None:
                await self._queue.put(tool_call(tool_name, tool_input))

    def _extract_tool_payload(
        self,
        payload: dict[str, object],
    ) -> tuple[str | None, dict[str, Any] | None]:
        tool_name = payload.get("name", payload.get("tool"))
        tool_input = payload.get("input", payload.get("arguments", {}))
        if not isinstance(tool_name, str) or not isinstance(tool_input, dict):
            return None, None
        return tool_name, cast("dict[str, Any]", tool_input)

    def _extract_tool_result(
        self,
        payload: dict[str, object],
    ) -> tuple[str | None, str | None]:
        tool_name = payload.get("name", payload.get("tool"))
        tool_output = payload.get("output", payload.get("result"))
        if not isinstance(tool_name, str):
            return None, None
        if isinstance(tool_output, str):
            return tool_name, tool_output
        if tool_output is None:
            return tool_name, ""
        return tool_name, json.dumps(tool_output, separators=(",", ":"))
