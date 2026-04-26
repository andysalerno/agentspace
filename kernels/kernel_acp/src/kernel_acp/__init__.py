"""ACP kernel - wraps any Agent Client Protocol stdio server."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import shlex
import uuid
from dataclasses import replace
from typing import TYPE_CHECKING, cast

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
DEFAULT_ACP_COMMAND = "opencode acp"
PROTOCOL_VERSION = 1
_STREAM_BUFFER_LIMIT = 16 * 1024 * 1024


class AcpKernel:
    """Kernel that speaks ACP JSON-RPC over stdio to a compliant agent server."""

    def __init__(self) -> None:
        self._status = KernelStatus.IDLE
        self._session_id = ""
        self._config = KernelConfig()
        self._process: asyncio.subprocess.Process | None = None
        self._stdout_task: asyncio.Task[None] | None = None
        self._stderr_task: asyncio.Task[None] | None = None
        self._queue: asyncio.Queue[KernelEvent | None] = asyncio.Queue()
        self._raw_lines: list[str] = []
        self._next_request_id = 0
        self._pending: dict[int, asyncio.Future[object]] = {}
        self._agent_capabilities: dict[str, object] = {}
        self._tool_names: dict[str, str] = {}

    @property
    def name(self) -> str:
        return "acp"

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
        return self._config.env.get("KERNEL_ACP_WORKSPACE_DIR", DEFAULT_WORKSPACE_DIR)

    async def start(self, config: KernelConfig) -> None:
        self._config = config
        self._session_id = config.session_id or uuid.uuid4().hex[:12]
        self._status = KernelStatus.IDLE
        self._raw_lines = []
        self._agent_capabilities = {}
        self._tool_names = {}

        try:
            cmd = self._build_command()
        except ValueError as exc:
            await self._queue.put(error(str(exc)))
            await self._finish(KernelStatus.ERROR)
            return
        env = self._build_env()
        cwd = self._workspace_dir

        logger.info("spawning ACP subprocess: cmd=%s cwd=%s", cmd, cwd)

        try:
            self._process = await asyncio.create_subprocess_exec(
                *cmd,
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env,
                cwd=cwd,
                limit=_STREAM_BUFFER_LIMIT,
            )
        except FileNotFoundError:
            await self._queue.put(error(f"ACP server command not found: {cmd[0]}"))
            await self._finish(KernelStatus.ERROR)
            return
        except OSError as exc:
            await self._queue.put(error(f"failed to start ACP server: {exc}"))
            await self._finish(KernelStatus.ERROR)
            return

        self._stdout_task = asyncio.create_task(self._read_stdout())
        self._stderr_task = asyncio.create_task(self._read_stderr())

        try:
            await self._initialize()
            await self._setup_session()
        except RuntimeError as exc:
            await self._queue.put(error(str(exc)))
            await self._finish(KernelStatus.ERROR)
            return

        await self._queue.put(session_start(self._session_id, self.name))

    async def send(self, message: str) -> None:
        if self._process is None or self._process.returncode is not None:
            await self._queue.put(error("ACP server is not running"))
            await self._finish(KernelStatus.ERROR)
            return

        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        try:
            await self._request(
                "session/prompt",
                {
                    "sessionId": self._session_id,
                    "prompt": [{"type": "text", "text": message}],
                },
            )
        except RuntimeError as exc:
            await self._queue.put(error(str(exc)))
            await self._finish(KernelStatus.ERROR)
            return

        self._status = KernelStatus.IDLE
        await self._queue.put(status_event(KernelStatus.IDLE))
        await self._finish(KernelStatus.DONE)

    async def recv(self) -> AsyncIterator[KernelEvent]:
        while True:
            event = await self._queue.get()
            if event is None:
                return
            yield event

    async def stop(self) -> None:
        await self._stop_process()
        self._status = KernelStatus.DONE

    def _build_command(self) -> list[str]:
        raw = self._config.env.get("KERNEL_ACP_COMMAND", DEFAULT_ACP_COMMAND)
        cmd = shlex.split(raw)
        if not cmd:
            msg = "KERNEL_ACP_COMMAND must contain an executable"
            raise ValueError(msg)

        extra_args = self._config.env.get("KERNEL_ACP_EXTRA_ARGS", "")
        for arg in extra_args.splitlines():
            if arg:
                cmd.append(arg)
        return cmd

    def _build_env(self) -> dict[str, str]:
        env = {**os.environ}
        env.update({key: value for key, value in self._config.env.items() if value})
        return env

    async def _initialize(self) -> None:
        result = await self._request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "agentspace",
                    "title": "AgentSpace",
                    "version": "0.1.0",
                },
            },
        )
        result_dict = self._as_dict(result)
        protocol_version = result_dict.get("protocolVersion")
        if protocol_version != PROTOCOL_VERSION:
            msg = f"ACP protocol version mismatch: server chose {protocol_version!r}"
            raise RuntimeError(msg)
        self._agent_capabilities = self._as_dict(
            result_dict.get("agentCapabilities"),
        )

    async def _setup_session(self) -> None:
        params: dict[str, object] = {
            "cwd": self._workspace_dir,
            "mcpServers": self._mcp_servers(),
        }
        resume_session = self._config.session_id or self._config.env.get(
            "KERNEL_ACP_SESSION_ID",
        )

        if resume_session and self._supports_resume():
            result = await self._request(
                "session/resume",
                {"sessionId": resume_session, **params},
            )
            self._session_id = resume_session
            self._config = replace(self._config, session_id=resume_session)
            self._capture_session_id(result)
            return

        if resume_session and self._agent_capabilities.get("loadSession") is True:
            await self._request(
                "session/load",
                {"sessionId": resume_session, **params},
            )
            self._session_id = resume_session
            self._config = replace(self._config, session_id=resume_session)
            return

        result = await self._request("session/new", params)
        self._capture_session_id(result)

    def _supports_resume(self) -> bool:
        session_capabilities = self._as_dict(
            self._agent_capabilities.get("sessionCapabilities"),
        )
        return isinstance(session_capabilities.get("resume"), dict)

    def _mcp_servers(self) -> list[object]:
        raw = self._config.env.get("KERNEL_ACP_MCP_SERVERS")
        if not raw:
            return []
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError as exc:
            msg = f"KERNEL_ACP_MCP_SERVERS must be valid JSON: {exc.msg}"
            raise RuntimeError(msg) from exc
        if not isinstance(parsed, list):
            msg = "KERNEL_ACP_MCP_SERVERS must be a JSON array"
            raise TypeError(msg)
        return cast("list[object]", parsed)

    async def _request(self, method: str, params: dict[str, object]) -> object:
        request_id = self._next_request_id
        self._next_request_id += 1
        loop = asyncio.get_running_loop()
        future: asyncio.Future[object] = loop.create_future()
        self._pending[request_id] = future
        await self._write_message(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params,
            },
        )
        return await future

    async def _respond(self, request_id: object, result: object) -> None:
        await self._write_message(
            {"jsonrpc": "2.0", "id": request_id, "result": result},
        )

    async def _respond_error(
        self,
        request_id: object,
        code: int,
        message: str,
    ) -> None:
        await self._write_message(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": code, "message": message},
            },
        )

    async def _write_message(self, message: dict[str, object]) -> None:
        if self._process is None or self._process.stdin is None:
            msg = "ACP server stdin is not available"
            raise RuntimeError(msg)
        payload = json.dumps(message, separators=(",", ":")) + "\n"
        self._process.stdin.write(payload.encode())
        await self._process.stdin.drain()

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
                await self._queue.put(error(f"invalid ACP JSON-RPC message: {line}"))
                continue
            if isinstance(obj, dict):
                await self._handle_message(cast("dict[str, object]", obj))
            else:
                await self._queue.put(error("invalid ACP JSON-RPC message"))

    async def _read_stderr(self) -> None:
        if self._process is None or self._process.stderr is None:
            return
        async for raw_line in self._process.stderr:
            line = raw_line.decode().rstrip("\n").rstrip("\r").strip()
            if line:
                self._raw_lines.append(f"[stderr] {line}")
                await self._queue.put(error(line))

    async def _handle_message(self, obj: dict[str, object]) -> None:
        request_id = obj.get("id")
        if "result" in obj or "error" in obj:
            await self._handle_response(obj)
            return

        method = obj.get("method")
        if not isinstance(method, str):
            return

        params = self._as_dict(obj.get("params"))
        if method == "session/update":
            await self._map_session_update(params)
            return

        if request_id is None:
            return

        if method == "session/request_permission":
            await self._respond(request_id, self._permission_response(params))
            return

        await self._respond_error(
            request_id,
            -32601,
            f"unsupported ACP method: {method}",
        )

    async def _handle_response(self, obj: dict[str, object]) -> None:
        request_id = obj.get("id")
        if not isinstance(request_id, int):
            return
        future = self._pending.pop(request_id, None)
        if future is None or future.done():
            return
        error_obj = self._as_dict(obj.get("error"))
        if error_obj:
            message = error_obj.get("message")
            detail = message if isinstance(message, str) else json.dumps(error_obj)
            future.set_exception(RuntimeError(f"ACP request failed: {detail}"))
            return
        future.set_result(obj.get("result"))

    async def _map_session_update(self, params: dict[str, object]) -> None:
        session_id = params.get("sessionId")
        if isinstance(session_id, str) and session_id:
            self._session_id = session_id
            self._config = replace(self._config, session_id=session_id)

        update = self._as_dict(params.get("update"))
        update_type = update.get("sessionUpdate")

        if update_type in {"agent_message_chunk", "user_message_chunk"}:
            text = self._content_text(update.get("content"))
            if text and update_type == "agent_message_chunk":
                await self._queue.put(text_delta(text))
            return

        if update_type == "agent_thought_chunk":
            text = self._content_text(update.get("content"))
            if text:
                await self._queue.put(reasoning_delta(text))
            return

        if update_type == "tool_call":
            await self._map_tool_call(update)
            return

        if update_type == "tool_call_update":
            await self._map_tool_call_update(update)
            return

        if update_type == "plan":
            entries = update.get("entries")
            await self._queue.put(
                reasoning_delta(
                    json.dumps({"plan": entries}, separators=(",", ":")),
                ),
            )

    async def _map_tool_call(self, update: dict[str, object]) -> None:
        tool_call_id = update.get("toolCallId")
        title = update.get("title")
        tool_name = title if isinstance(title, str) and title else "tool"
        if isinstance(tool_call_id, str):
            self._tool_names[tool_call_id] = tool_name
        raw_input = self._as_dict(update.get("rawInput"))
        await self._queue.put(tool_call(tool_name, raw_input))

        status = update.get("status")
        if status in {"completed", "failed"}:
            await self._emit_tool_result(tool_name, update)

    async def _map_tool_call_update(self, update: dict[str, object]) -> None:
        tool_call_id = update.get("toolCallId")
        tool_name = "tool"
        if isinstance(tool_call_id, str):
            tool_name = self._tool_names.get(tool_call_id, tool_name)
        title = update.get("title")
        if isinstance(title, str) and title:
            tool_name = title
            if isinstance(tool_call_id, str):
                self._tool_names[tool_call_id] = tool_name

        raw_input = self._as_dict(update.get("rawInput"))
        if raw_input:
            await self._queue.put(tool_call(tool_name, raw_input))

        status = update.get("status")
        if status in {"completed", "failed", "cancelled"}:
            await self._emit_tool_result(tool_name, update)

    async def _emit_tool_result(
        self,
        tool_name: str,
        update: dict[str, object],
    ) -> None:
        raw_output = update.get("rawOutput")
        output = self._stringify_output(raw_output)
        if not output:
            output = self._tool_content_text(update.get("content"))
        status = update.get("status")
        if isinstance(status, str) and status != "completed":
            output = "\n".join(part for part in (output, f"[status: {status}]") if part)
        await self._queue.put(tool_result(tool_name, output))

    def _permission_response(self, params: dict[str, object]) -> dict[str, object]:
        preferred = self._config.env.get("KERNEL_ACP_PERMISSION_OPTION")
        options = params.get("options")
        if isinstance(options, list):
            option_id = self._select_permission_option(
                cast("list[object]", options),
                preferred,
            )
            if option_id:
                return {"outcome": {"outcome": "selected", "optionId": option_id}}
        return {"outcome": {"outcome": "cancelled"}}

    def _select_permission_option(
        self,
        options: list[object],
        preferred: str | None,
    ) -> str | None:
        option_ids: list[str] = []
        for option in options:
            option_dict = self._as_dict(option)
            option_id = option_dict.get("optionId")
            if not isinstance(option_id, str):
                option_id = option_dict.get("id")
            if isinstance(option_id, str):
                option_ids.append(option_id)
        if preferred and preferred in option_ids:
            return preferred
        for option_id in option_ids:
            if option_id in {"allow_once", "allow_always", "allow"}:
                return option_id
        return option_ids[0] if option_ids else None

    def _capture_session_id(self, result: object) -> None:
        result_dict = self._as_dict(result)
        session_id = result_dict.get("sessionId")
        if isinstance(session_id, str) and session_id:
            self._session_id = session_id
            self._config = replace(self._config, session_id=session_id)

    def _content_text(self, content: object) -> str:
        content_dict = self._as_dict(content)
        content_type = content_dict.get("type")
        if content_type == "text":
            text = content_dict.get("text")
            return text if isinstance(text, str) else ""
        if content_type == "content":
            return self._content_text(content_dict.get("content"))
        return self._stringify_output(content)

    def _tool_content_text(self, content: object) -> str:
        if isinstance(content, list):
            content_items = cast("list[object]", content)
            return "".join(self._content_text(item) for item in content_items)
        return self._content_text(content)

    def _stringify_output(self, raw_output: object) -> str:
        if isinstance(raw_output, str):
            return raw_output
        if raw_output is None:
            return ""
        return json.dumps(raw_output, separators=(",", ":"))

    def _as_dict(self, value: object) -> dict[str, object]:
        if isinstance(value, dict):
            return cast("dict[str, object]", value)
        return {}

    async def _finish(self, status: KernelStatus) -> None:
        self._status = status
        if status != KernelStatus.ERROR:
            await self._queue.put(status_event(KernelStatus.DONE))
        else:
            await self._queue.put(status_event(KernelStatus.ERROR))
            await self._queue.put(status_event(KernelStatus.DONE))
        await self._queue.put(session_end())
        await self._queue.put(None)
        await self._stop_process()

    async def _stop_process(self) -> None:
        for future in self._pending.values():
            if not future.done():
                future.set_exception(RuntimeError("ACP server stopped"))
        self._pending = {}

        if self._process is not None and self._process.returncode is None:
            self._process.terminate()
            try:
                await asyncio.wait_for(self._process.wait(), timeout=5.0)
            except TimeoutError:
                self._process.kill()
                await self._process.wait()

        tasks = [task for task in (self._stdout_task, self._stderr_task) if task]
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
