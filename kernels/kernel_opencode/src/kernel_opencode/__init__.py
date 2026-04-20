"""OpenCode CLI kernel — wraps `opencode` as an agent harness."""

from __future__ import annotations

import asyncio
import json
import logging
import os
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

CUSTOM_AGENT_NAME = "custom"
CUSTOM_AGENT_PATH = (
    Path.home() / ".config" / "opencode" / "agents" / f"{CUSTOM_AGENT_NAME}.md"
)

# asyncio's default StreamReader limit is 64 KiB; opencode JSONL events can
# easily exceed that when a tool result embeds a fetched web page or other
# large payload. Bump generously to avoid mid-stream readline() failures.
_STREAM_BUFFER_LIMIT = 16 * 1024 * 1024


class OpenCodeKernel:
    """Kernel that wraps OpenCode CLI (`opencode run`)."""

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
        return "opencode"

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
        return self._config.env.get("OPENCODE_WORKSPACE_DIR", DEFAULT_WORKSPACE_DIR)

    async def start(self, config: KernelConfig) -> None:
        self._config = config
        self._session_id = config.session_id or uuid.uuid4().hex[:12]
        self._status = KernelStatus.IDLE
        self._raw_lines = []
        await self._queue.put(session_start(self._session_id, self.name))

    async def send(self, message: str) -> None:
        if self._output_task is not None and not self._output_task.done():
            await self._queue.put(
                error("opencode kernel is already processing a request"),
            )
            return

        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        try:
            self._write_provider_config()
            self._write_custom_agent_prompt()
        except ValueError as exc:
            await self._queue.put(error(str(exc)))
            await self._finish(KernelStatus.ERROR)
            return
        except OSError as exc:
            logger.exception("failed to write opencode provider config")
            detail = exc.strerror or type(exc).__name__
            await self._queue.put(
                error(f"failed to write opencode provider config: {detail}"),
            )
            await self._finish(KernelStatus.ERROR)
            return

        cmd = self._build_command(message)
        env = self._build_env()
        cwd = self._workspace_dir

        logger.info("spawning opencode subprocess: cmd=%s cwd=%s", cmd, cwd)

        try:
            self._process = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env,
                cwd=cwd,
                # opencode emits a single JSON object per line, and tool
                # results (e.g. webfetch payloads) can easily exceed the
                # asyncio StreamReader default of 64 KiB, which would cause
                # readline() to raise "Separator is found, but chunk is
                # longer than limit" and abort the stream mid-response.
                limit=_STREAM_BUFFER_LIMIT,
            )
        except FileNotFoundError:
            await self._queue.put(error("opencode CLI not found; is it installed?"))
            await self._finish(KernelStatus.ERROR)
            return
        except OSError as exc:
            await self._queue.put(error(f"failed to start opencode CLI: {exc}"))
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
            "opencode",
            "run",
            message,
            "--format",
            "json",
            "--dangerously-skip-permissions",
            "--thinking",
        ]

        model = self._config.env.get("OPENCODE_MODEL")
        if model:
            cmd.extend(["--model", model])

        variant = self._config.env.get("OPENCODE_VARIANT")
        if variant:
            cmd.extend(["--variant", variant])

        agent = self._config.env.get("OPENCODE_AGENT")
        if not agent and self._has_custom_agent_prompt():
            agent = CUSTOM_AGENT_NAME
        if agent:
            cmd.extend(["--agent", agent])

        resume_session = self._config.session_id or self._config.env.get(
            "OPENCODE_SESSION_ID",
        )
        if resume_session:
            cmd.extend(["--session", resume_session])

        cwd = self._workspace_dir
        cmd.extend(["--dir", cwd])

        extra_args = self._config.env.get("OPENCODE_EXTRA_ARGS", "")
        for arg in extra_args.splitlines():
            if arg:
                cmd.append(arg)

        return cmd

    def _write_provider_config(self) -> None:
        """Write the opencode provider config to ~/.config/opencode/opencode.json.

        Pulls the base URL, API key, and model name from environment
        variables forwarded via the kernel template. Raises ``ValueError``
        if any of the required variables is missing or empty so the failure
        is surfaced to the client instead of producing a confusing
        downstream auth error.
        """
        env_get = self._config.env.get
        required = {
            "KERNEL_OPENCODE_BASE_URL": env_get("KERNEL_OPENCODE_BASE_URL"),
            "KERNEL_OPENCODE_API_KEY": env_get("KERNEL_OPENCODE_API_KEY"),
            "KERNEL_OPENCODE_MODEL_NAME": env_get("KERNEL_OPENCODE_MODEL_NAME"),
        }
        missing = [name for name, value in required.items() if not value]
        if missing:
            msg = (
                "opencode kernel is missing required environment "
                f"variable(s): {', '.join(missing)}. Set them on the agent's "
                "Environment Variables field."
            )
            raise ValueError(msg)

        base_url = required["KERNEL_OPENCODE_BASE_URL"]
        api_key = required["KERNEL_OPENCODE_API_KEY"]
        model_name = required["KERNEL_OPENCODE_MODEL_NAME"]

        config = {
            "$schema": "https://opencode.ai/config.json",
            "provider": {
                "customprovider": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "customprovider",
                    "options": {
                        "baseURL": base_url,
                        "apiKey": api_key,
                    },
                    "models": {
                        model_name: {
                            "name": model_name,
                        },
                    },
                },
            },
            "permission": {
                "*": "allow",
                "bash": {
                    "*": "allow",
                },
                "webfetch": "deny",
                "doom_loop": "deny",
                "external_directory": "deny",
                "websearch": "deny",
                "question": "deny",
                "lsp": "deny",
            },
        }

        config_path = Path.home() / ".config" / "opencode" / "opencode.json"
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps(config, indent=2))
        logger.info("wrote opencode provider config to %s", config_path)

    def _write_custom_agent_prompt(self) -> None:
        """Write the agent's system prompt to opencode's custom agent file.

        The system prompt is forwarded by the client_service via the
        ``KERNEL_SYSTEM_PROMPT`` env var. The file is always written (even
        when empty) so a stale prompt from a previous run cannot leak
        into the current session.
        """
        prompt = self._config.env.get("KERNEL_SYSTEM_PROMPT", "")
        CUSTOM_AGENT_PATH.parent.mkdir(parents=True, exist_ok=True)
        CUSTOM_AGENT_PATH.write_text(prompt)
        logger.info(
            "wrote opencode custom agent prompt to %s (%d chars)",
            CUSTOM_AGENT_PATH,
            len(prompt),
        )

    def _has_custom_agent_prompt(self) -> bool:
        """Return True if the custom agent prompt file has non-whitespace content."""
        try:
            return bool(CUSTOM_AGENT_PATH.read_text().strip())
        except OSError:
            return False

    def _build_env(self) -> dict[str, str]:
        env = {**os.environ}
        for key in (
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "OPENCODE_SERVER_PASSWORD",
        ):
            value = self._config.env.get(key)
            if value:
                env[key] = value
        return env

    async def _read_output(self) -> None:
        if self._process is None:
            return

        stdout_task = asyncio.create_task(self._read_stdout())
        stderr_task = asyncio.create_task(self._read_stderr())
        await asyncio.gather(stdout_task, stderr_task)

        returncode = await self._process.wait()
        if returncode != 0:
            await self._queue.put(error(f"opencode exited with code {returncode}"))
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

    async def _map_event(self, obj: dict[str, object]) -> None:
        event_type = obj.get("type", "")
        raw_part = obj.get("part")
        part = cast("dict[str, object]", raw_part) if isinstance(raw_part, dict) else {}

        session_id = obj.get("sessionID")
        if isinstance(session_id, str) and session_id:
            self._session_id = session_id
            self._config = replace(self._config, session_id=session_id)

        if event_type == "step_start":
            logger.debug("step started")
            return

        if event_type == "text":
            text = part.get("text", "")
            if isinstance(text, str) and text:
                await self._queue.put(text_delta(text))
            return

        if event_type == "reasoning":
            text = part.get("text", "")
            if isinstance(text, str) and text:
                await self._queue.put(reasoning_delta(text))
            return

        if event_type == "tool_use":
            await self._map_tool_use(part)
            return

        if event_type == "step_finish":
            reason = part.get("reason", "")
            if reason == "stop":
                self._status = KernelStatus.IDLE
                await self._queue.put(status_event(KernelStatus.IDLE))
            return

        logger.debug("unhandled opencode event: %s", event_type)

    async def _map_tool_use(self, part: dict[str, object]) -> None:
        tool_name = part.get("tool")
        if not isinstance(tool_name, str):
            return

        raw_state = part.get("state")
        state = (
            cast("dict[str, object]", raw_state) if isinstance(raw_state, dict) else {}
        )

        # Emit tool_call with input
        raw_input = state.get("input")
        tool_input: dict[str, Any] = {}
        if isinstance(raw_input, dict):
            tool_input = cast("dict[str, Any]", raw_input)
        await self._queue.put(tool_call(tool_name, tool_input))

        # Emit tool_result with output
        status = state.get("status")
        if status == "completed":
            output = state.get("output", "")
            out_str = (
                output
                if isinstance(output, str)
                else json.dumps(output, separators=(",", ":"))
            )
            await self._queue.put(tool_result(tool_name, out_str))
