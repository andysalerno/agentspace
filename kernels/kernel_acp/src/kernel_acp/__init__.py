"""ACP kernel - wraps any Agent Client Protocol stdio server."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import shlex
import uuid
from dataclasses import dataclass, replace
from enum import StrEnum
from pathlib import Path
from time import perf_counter
from typing import TYPE_CHECKING, cast

from kernel.events import (
    KernelEvent,
    KernelStatus,
    error,
    session_end,
    session_prompt_result,
    session_start,
    session_update,
    status_event,
)
from kernel.protocol import KernelConfig

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

logger = logging.getLogger(__name__)

DEFAULT_WORKSPACE_DIR = "/workspace"
DEFAULT_ACP_SERVER = "opencode"
OPENCODE_CUSTOM_AGENT_NAME = "custom"
OPENCODE_CUSTOM_AGENT_PATH = (
    Path.home() / ".config" / "opencode" / "agents" / f"{OPENCODE_CUSTOM_AGENT_NAME}.md"
)
COPILOT_CUSTOM_AGENT_NAME = "agentspace"
COPILOT_RUNTIME_ROOT = Path("/tmp/agentspace-copilot")  # noqa: S108
COPILOT_EXPERIMENTAL_ENV = "KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED"
PROTOCOL_VERSION = 1
_STREAM_BUFFER_LIMIT = 16 * 1024 * 1024
_DEFAULT_TERMINAL_OUTPUT_LIMIT = 1024 * 1024
_UNHANDLED: object = object()
_DEFAULT_API_FLAVOR = "chat_completions"
_OPENCODE_PROVIDER_NPM_BY_API_FLAVOR = {
    "chat_completions": "@ai-sdk/openai-compatible",
    "responses": "@ai-sdk/openai",
}
_OPENCODE_PERMISSION_CONFIG = {
    "*": "allow",
    "bash": {
        "*": "allow",
    },
    "webfetch": "deny",
    "doom_loop": "deny",
    "external_directory": {
        "*": "deny",
        "/tmp/**": "allow",  # noqa: S108
    },
    "websearch": "deny",
    "question": "deny",
    "lsp": "deny",
}
_COPILOT_SECRET_ENV_VARS = (
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_BEARER_TOKEN",
    "COPILOT_PROVIDER_HEADERS",
)
_CONNECTION_SECRET_ENV_VARS = (
    "CONNECTION_API_KEY",
    "CONNECTION_BEARER_TOKEN",
    "CONNECTION_HEADERS",
)
_GITHUB_AUTH_ENV_VARS = (
    "COPILOT_GITHUB_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
)
_COPILOT_MODEL_ENV_MAP = {
    "KERNEL_ACP_PROVIDER_MODEL_ID": "COPILOT_PROVIDER_MODEL_ID",
    "KERNEL_ACP_PROVIDER_WIRE_MODEL": "COPILOT_PROVIDER_WIRE_MODEL",
    "KERNEL_ACP_MAX_PROMPT_TOKENS": "COPILOT_PROVIDER_MAX_PROMPT_TOKENS",
    "KERNEL_ACP_MAX_OUTPUT_TOKENS": "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS",
}
_COPILOT_CONNECTION_ENV_MAP = {
    "CONNECTION_API_KEY": "COPILOT_PROVIDER_API_KEY",
    "CONNECTION_BEARER_TOKEN": "COPILOT_PROVIDER_BEARER_TOKEN",
    "CONNECTION_TRANSPORT": "COPILOT_PROVIDER_TRANSPORT",
    "CONNECTION_AZURE_API_VERSION": "COPILOT_PROVIDER_AZURE_API_VERSION",
    "CONNECTION_HEADERS": "COPILOT_PROVIDER_HEADERS",
}
_COPILOT_WIRE_API_BY_CONNECTION_FLAVOR = {
    "chat_completions": "completions",
    "responses": "responses",
}


class AcpServer(StrEnum):
    OPENCODE = "opencode"
    COPILOT = "copilot"
    CUSTOM = "custom"


class AcpRequestError(ValueError):
    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


@dataclass(slots=True)
class TerminalSession:
    process: asyncio.subprocess.Process
    output_limit: int
    output: str = ""
    truncated: bool = False
    reader_task: asyncio.Task[None] | None = None


class AcpKernel:
    """Kernel that speaks ACP JSON-RPC over stdio to a compliant agent server."""

    supports_persistent_process = True

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
        self._terminals: dict[str, TerminalSession] = {}

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
        started_at = perf_counter()
        self._config = config
        self._session_id = config.session_id or uuid.uuid4().hex[:12]
        self._status = KernelStatus.IDLE
        self._raw_lines = []
        self._agent_capabilities = {}
        self._terminals = {}

        try:
            cmd = self._build_command()
            self._prepare_server()
            env = self._build_env()
        except ValueError as exc:
            await self._queue.put(error(str(exc)))
            await self._finish(KernelStatus.ERROR)
            return
        except OSError as exc:
            logger.exception("failed to prepare ACP server")
            detail = exc.strerror or type(exc).__name__
            await self._queue.put(
                error(f"failed to prepare ACP server: {detail}"),
            )
            await self._finish(KernelStatus.ERROR)
            return
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
            logger.info(
                "ACP subprocess spawned: elapsed_ms=%.1f",
                (perf_counter() - started_at) * 1000,
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
            await self._initialize_agent_session(started_at)
        except asyncio.CancelledError:
            await self._stop_process()
            raise
        except RuntimeError as exc:
            await self._queue.put(error(str(exc)))
            await self._finish(KernelStatus.ERROR)
            return

        await self._queue.put(session_start(self._session_id, self.name))

    async def _initialize_agent_session(self, started_at: float) -> None:
        initialize_started_at = perf_counter()
        await self._initialize()
        logger.info(
            "ACP initialize completed: elapsed_ms=%.1f total_ms=%.1f",
            (perf_counter() - initialize_started_at) * 1000,
            (perf_counter() - started_at) * 1000,
        )
        setup_started_at = perf_counter()
        await self._setup_session()
        logger.info(
            "ACP session setup completed: elapsed_ms=%.1f total_ms=%.1f session=%s",
            (perf_counter() - setup_started_at) * 1000,
            (perf_counter() - started_at) * 1000,
            self._session_id,
        )

    async def send(self, message: str) -> None:
        started_at = perf_counter()
        if self._process is None or self._process.returncode is not None:
            await self._queue.put(error("ACP server is not running"))
            await self._finish(KernelStatus.ERROR)
            return

        self._status = KernelStatus.BUSY
        await self._queue.put(status_event(KernelStatus.BUSY))

        try:
            logger.info(
                "ACP sending session/prompt: session=%s message_chars=%d",
                self._session_id,
                len(message),
            )
            result = await self._request(
                "session/prompt",
                {
                    "sessionId": self._session_id,
                    "prompt": [{"type": "text", "text": message}],
                },
            )
            result_dict = self._as_dict(result)
            await self._queue.put(
                session_prompt_result(self._session_id, result_dict),
            )
            logger.info(
                "ACP session/prompt completed: session=%s elapsed_ms=%.1f",
                self._session_id,
                (perf_counter() - started_at) * 1000,
            )
        except RuntimeError as exc:
            await self._queue.put(error(str(exc)))
            await self._finish(KernelStatus.ERROR)
            return

        self._status = KernelStatus.IDLE
        await self._queue.put(status_event(KernelStatus.IDLE))
        await self._finish_turn(KernelStatus.DONE)

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
        server = self._acp_server()
        if server == AcpServer.OPENCODE:
            return ["opencode", "acp"]
        if server == AcpServer.COPILOT:
            self._require_copilot_experimental_enabled()
            cmd = [
                "copilot",
                "--acp",
                "--yolo",
                "--disable-builtin-mcps",
                "--no-auto-update",
                f"--secret-env-vars={','.join(_COPILOT_SECRET_ENV_VARS)}",
            ]
            if self._config.env.get("KERNEL_SYSTEM_PROMPT", "").strip():
                cmd.extend(["--agent", COPILOT_CUSTOM_AGENT_NAME])
            return cmd

        raw = self._config.env.get("KERNEL_ACP_COMMAND", "")
        cmd = shlex.split(raw)
        if not cmd:
            msg = (
                "KERNEL_ACP_COMMAND must contain an executable when "
                "KERNEL_ACP_SERVER=custom"
            )
            raise ValueError(msg)

        extra_args = self._config.env.get("KERNEL_ACP_EXTRA_ARGS", "")
        for arg in extra_args.splitlines():
            if arg:
                cmd.append(arg)

        return cmd

    def _acp_server(self) -> AcpServer:
        raw = self._config.env.get("KERNEL_ACP_SERVER", DEFAULT_ACP_SERVER)
        try:
            return AcpServer(raw)
        except ValueError as exc:
            valid = ", ".join(server.value for server in AcpServer)
            msg = f"KERNEL_ACP_SERVER must be one of: {valid}"
            raise ValueError(msg) from exc

    def _require_copilot_experimental_enabled(self) -> None:
        raw = self._config.env.get(COPILOT_EXPERIMENTAL_ENV, "")
        if raw.casefold() in {"1", "true", "yes", "on"}:
            return
        msg = (
            "Copilot ACP support is disabled because Copilot CLI 1.0.73 requires "
            "GitHub authentication for ACP sessions even with offline BYOK "
            "configuration (github/copilot-cli#4016). Set "
            f"{COPILOT_EXPERIMENTAL_ENV}=true only to test the experimental "
            "integration before the upstream fix is available."
        )
        raise ValueError(msg)

    def _prepare_server(self) -> None:
        server = self._acp_server()
        if server == AcpServer.OPENCODE:
            self._write_opencode_config()
            self._write_opencode_custom_agent_prompt()
        elif server == AcpServer.COPILOT:
            self._write_copilot_custom_agent_prompt()
            self._link_copilot_skills()

    def _build_env(self) -> dict[str, str]:
        env = {**os.environ}
        env.update({key: value for key, value in self._config.env.items() if value})
        server = self._acp_server()
        if server == AcpServer.OPENCODE and self._has_opencode_custom_agent_prompt():
            env["OPENCODE_CONFIG_CONTENT"] = self._opencode_config_content(
                env.get("OPENCODE_CONFIG_CONTENT"),
            )
        elif server == AcpServer.COPILOT:
            env = self._build_copilot_env(env)
        return env

    def _build_copilot_env(self, env: dict[str, str]) -> dict[str, str]:
        source = self._config.env
        base_url = source.get("CONNECTION_URL")
        model = source.get("KERNEL_ACP_MODEL_NAME")
        missing = [
            name
            for name, value in (
                ("CONNECTION_URL", base_url),
                ("KERNEL_ACP_MODEL_NAME", model),
            )
            if not value
        ]
        if missing:
            msg = (
                "Copilot ACP is missing required environment variable(s): "
                f"{', '.join(missing)}. Assign a Connection and model to the agent."
            )
            raise ValueError(msg)

        sanitized = {
            key: value
            for key, value in env.items()
            if not key.startswith("COPILOT_PROVIDER_")
            and key != "COPILOT_MODEL"
            and not key.startswith("CONNECTION_")
            and key not in _GITHUB_AUTH_ENV_VARS
        }
        sanitized["COPILOT_PROVIDER_BASE_URL"] = cast("str", base_url)
        sanitized["COPILOT_PROVIDER_TYPE"] = source.get(
            "CONNECTION_PROVIDER_TYPE",
            "openai",
        )
        api_flavor = source.get("CONNECTION_API_FLAVOR", _DEFAULT_API_FLAVOR)
        wire_api = _COPILOT_WIRE_API_BY_CONNECTION_FLAVOR.get(api_flavor)
        if wire_api is None:
            valid = ", ".join(_COPILOT_WIRE_API_BY_CONNECTION_FLAVOR)
            msg = f"CONNECTION_API_FLAVOR must be one of: {valid}"
            raise ValueError(msg)
        sanitized["COPILOT_PROVIDER_WIRE_API"] = wire_api
        sanitized["COPILOT_MODEL"] = cast("str", model)

        self._copy_mapped_env(source, sanitized, _COPILOT_CONNECTION_ENV_MAP)
        self._copy_mapped_env(source, sanitized, _COPILOT_MODEL_ENV_MAP)

        sanitized["COPILOT_OFFLINE"] = "true"
        sanitized["COPILOT_HOME"] = str(self._copilot_home())
        return sanitized

    def _copy_mapped_env(
        self,
        source: dict[str, str],
        target: dict[str, str],
        mapping: dict[str, str],
    ) -> None:
        for source_key, target_key in mapping.items():
            value = source.get(source_key)
            if value:
                target[target_key] = value

    def _copilot_home(self) -> Path:
        return COPILOT_RUNTIME_ROOT / self._session_id

    def _opencode_config_content(self, raw: str | None) -> str:
        config: dict[str, object] = {}
        if raw:
            parsed = json.loads(raw)
            if not isinstance(parsed, dict):
                msg = "OPENCODE_CONFIG_CONTENT must be a JSON object"
                raise ValueError(msg)
            config = cast("dict[str, object]", parsed)
        config["default_agent"] = OPENCODE_CUSTOM_AGENT_NAME
        return json.dumps(config, separators=(",", ":"))

    def _write_opencode_config(self) -> None:
        """Write opencode provider and permission config for opencode ACP servers."""
        env_get = self._config.env.get
        base_url = (
            env_get("CONNECTION_URL")
            or env_get("KERNEL_ACP_BASE_URL")
            or env_get("KERNEL_OPENCODE_BASE_URL")
        )
        api_key = (
            env_get("CONNECTION_API_KEY")
            or env_get("KERNEL_ACP_API_KEY")
            or env_get("KERNEL_OPENCODE_API_KEY")
        )
        model_name = env_get("KERNEL_ACP_MODEL_NAME") or env_get(
            "KERNEL_OPENCODE_MODEL_NAME",
        )
        required = {
            "CONNECTION_URL": base_url,
            "CONNECTION_API_KEY": api_key,
            "KERNEL_ACP_MODEL_NAME": model_name,
        }
        missing = [name for name, value in required.items() if not value]
        if missing:
            msg = (
                "ACP kernel is missing required environment "
                f"variable(s): {', '.join(missing)}. Assign a Connection with "
                "a URL and API key, and set KERNEL_ACP_MODEL_NAME on the agent "
                "or kernel configuration."
            )
            raise ValueError(msg)
        base_url = cast("str", base_url)
        api_key = cast("str", api_key)
        model_name = cast("str", model_name)

        config_path = Path.home() / ".config" / "opencode" / "opencode.json"
        config: dict[str, object] = {
            "$schema": "https://opencode.ai/config.json",
        }
        if config_path.exists():
            loaded = json.loads(config_path.read_text())
            if not isinstance(loaded, dict):
                msg = f"opencode config must be a JSON object: {config_path}"
                raise ValueError(msg)
            config = cast("dict[str, object]", loaded)
            config.setdefault("$schema", "https://opencode.ai/config.json")
        config["model"] = f"customprovider/{model_name}"
        config["provider"] = {
            "customprovider": {
                "npm": self._opencode_provider_npm(),
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
        }
        config["permission"] = _OPENCODE_PERMISSION_CONFIG

        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps(config, indent=2))
        logger.info("wrote opencode config to %s", config_path)

    def _opencode_provider_npm(self) -> str:
        api_flavor = (
            self._config.env.get("CONNECTION_API_FLAVOR")
            or self._config.env.get("KERNEL_ACP_API_FLAVOR")
            or _DEFAULT_API_FLAVOR
        )
        provider_npm = _OPENCODE_PROVIDER_NPM_BY_API_FLAVOR.get(api_flavor)
        if provider_npm is None:
            valid = ", ".join(_OPENCODE_PROVIDER_NPM_BY_API_FLAVOR)
            msg = f"CONNECTION_API_FLAVOR must be one of: {valid}"
            raise ValueError(msg)
        return provider_npm

    def _write_opencode_custom_agent_prompt(self) -> None:
        prompt = self._config.env.get("KERNEL_SYSTEM_PROMPT", "")
        OPENCODE_CUSTOM_AGENT_PATH.parent.mkdir(parents=True, exist_ok=True)
        content = ""
        if prompt.strip():
            content = (
                "---\n"
                "description: AgentSpace custom system prompt\n"
                "mode: primary\n"
                "---\n"
                f"{prompt}"
            )
        OPENCODE_CUSTOM_AGENT_PATH.write_text(content)
        logger.info(
            "wrote opencode custom agent prompt to %s (%d chars)",
            OPENCODE_CUSTOM_AGENT_PATH,
            len(prompt),
        )

    def _has_opencode_custom_agent_prompt(self) -> bool:
        try:
            return bool(OPENCODE_CUSTOM_AGENT_PATH.read_text().strip())
        except OSError:
            return False

    def _write_copilot_custom_agent_prompt(self) -> None:
        prompt = self._config.env.get("KERNEL_SYSTEM_PROMPT", "")
        if not prompt.strip():
            return
        path = self._copilot_home() / "agents" / f"{COPILOT_CUSTOM_AGENT_NAME}.agent.md"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            "---\n"
            "name: agentspace\n"
            "description: AgentSpace session agent\n"
            "---\n"
            f"{prompt}",
            encoding="utf-8",
        )
        logger.info(
            "wrote Copilot custom agent prompt to %s (%d chars)",
            path,
            len(prompt),
        )

    def _link_copilot_skills(self) -> None:
        source_raw = self._config.env.get("KERNEL_SKILLS_DIR")
        if not source_raw:
            return
        source = Path(source_raw)
        if not source.is_dir():
            return
        target = self._copilot_home() / "skills"
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.is_symlink():
            if target.resolve() == source.resolve():
                return
            msg = f"Copilot skills link points to an unexpected path: {target}"
            raise ValueError(msg)
        if target.exists():
            msg = f"Copilot skills path already exists and is not a symlink: {target}"
            raise ValueError(msg)
        target.symlink_to(source.resolve(), target_is_directory=True)
        logger.info("linked Copilot skills %s -> %s", target, source)

    async def _initialize(self) -> None:
        result = await self._request(
            "initialize",
            {
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": {
                        "readTextFile": True,
                        "writeTextFile": True,
                    },
                    "terminal": True,
                },
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
        write_started_at = perf_counter()
        await self._write_message(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params,
            },
        )
        if method == "session/prompt":
            logger.info(
                "ACP session/prompt stdin write: request_id=%d elapsed_ms=%.1f",
                request_id,
                (perf_counter() - write_started_at) * 1000,
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
                logger.debug("ACP stderr: %s", line)

    async def _handle_message(self, obj: dict[str, object]) -> None:  # noqa: PLR0911
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

        try:
            result = await self._handle_client_request(method, params)
        except AcpRequestError as exc:
            await self._respond_error(request_id, exc.code, exc.message)
            return
        if result is not _UNHANDLED:
            await self._respond(request_id, result)
            return

        await self._respond_error(
            request_id,
            -32601,
            f"unsupported ACP method: {method}",
        )

    async def _handle_client_request(  # noqa: PLR0911
        self,
        method: str,
        params: dict[str, object],
    ) -> object | None:
        if method == "fs/read_text_file":
            return self._read_text_file(params)
        if method == "fs/write_text_file":
            self._write_text_file(params)
            return None
        if method == "terminal/create":
            return await self._terminal_create(params)
        if method == "terminal/output":
            return self._terminal_output(params)
        if method == "terminal/wait_for_exit":
            return await self._terminal_wait_for_exit(params)
        if method == "terminal/kill":
            await self._terminal_kill(params)
            return None
        if method == "terminal/release":
            await self._terminal_release(params)
            return None
        return _UNHANDLED

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
        await self._queue.put(session_update(self._session_id or None, update))

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

    def _read_text_file(self, params: dict[str, object]) -> dict[str, object]:
        path = self._workspace_path(params.get("path"))
        line = self._optional_int(params.get("line"), "line")
        limit = self._optional_int(params.get("limit"), "limit")
        if line is not None and line < 1:
            raise AcpRequestError(-32602, "line must be 1 or greater")
        if limit is not None and limit < 0:
            raise AcpRequestError(-32602, "limit must be 0 or greater")
        try:
            content = path.read_text(encoding="utf-8")
        except FileNotFoundError as exc:
            raise AcpRequestError(-32000, f"file not found: {path}") from exc
        except UnicodeDecodeError as exc:
            raise AcpRequestError(-32000, f"file is not valid UTF-8: {path}") from exc
        except OSError as exc:
            raise AcpRequestError(-32000, f"failed to read file: {exc}") from exc

        if line is not None or limit is not None:
            lines = content.splitlines(keepends=True)
            start = 0 if line is None else line - 1
            end = None if limit is None else start + limit
            content = "".join(lines[start:end])
        return {"content": content}

    def _write_text_file(self, params: dict[str, object]) -> None:
        path = self._workspace_path(params.get("path"))
        content = params.get("content")
        if not isinstance(content, str):
            raise AcpRequestError(-32602, "content must be a string")
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        except OSError as exc:
            raise AcpRequestError(-32000, f"failed to write file: {exc}") from exc

    async def _terminal_create(self, params: dict[str, object]) -> dict[str, object]:
        command = params.get("command")
        if not isinstance(command, str) or not command:
            raise AcpRequestError(-32602, "command must be a non-empty string")
        args = self._string_list(params.get("args"), "args")
        cwd_value = params.get("cwd")
        cwd = (
            self._workspace_path(cwd_value)
            if isinstance(cwd_value, str) and cwd_value
            else self._workspace_root()
        )
        output_limit = self._optional_int(
            params.get("outputByteLimit"),
            "outputByteLimit",
        )
        if output_limit is None:
            output_limit = _DEFAULT_TERMINAL_OUTPUT_LIMIT
        if output_limit < 0:
            raise AcpRequestError(-32602, "outputByteLimit must be 0 or greater")
        env = self._terminal_env(params.get("env"))

        try:
            process = await asyncio.create_subprocess_exec(
                command,
                *args,
                cwd=str(cwd),
                env=env,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
                limit=_STREAM_BUFFER_LIMIT,
            )
        except FileNotFoundError as exc:
            raise AcpRequestError(
                -32000,
                f"terminal command not found: {command}",
            ) from exc
        except OSError as exc:
            raise AcpRequestError(-32000, f"failed to start terminal: {exc}") from exc

        terminal_id = f"term_{uuid.uuid4().hex}"
        session = TerminalSession(process=process, output_limit=output_limit)
        session.reader_task = asyncio.create_task(self._read_terminal_output(session))
        self._terminals[terminal_id] = session
        return {"terminalId": terminal_id}

    async def _read_terminal_output(self, session: TerminalSession) -> None:
        if session.process.stdout is None:
            return
        async for chunk in session.process.stdout:
            text = chunk.decode("utf-8", errors="replace")
            session.output += text
            self._truncate_terminal_output(session)

    def _terminal_output(self, params: dict[str, object]) -> dict[str, object]:
        terminal = self._terminal(params)
        result: dict[str, object] = {
            "output": terminal.output,
            "truncated": terminal.truncated,
        }
        if terminal.process.returncode is not None:
            result["exitStatus"] = self._exit_status(terminal.process.returncode)
        return result

    async def _terminal_wait_for_exit(
        self,
        params: dict[str, object],
    ) -> dict[str, object]:
        terminal = self._terminal(params)
        returncode = await terminal.process.wait()
        if terminal.reader_task is not None:
            await terminal.reader_task
        return self._exit_status(returncode)

    async def _terminal_kill(self, params: dict[str, object]) -> None:
        terminal = self._terminal(params)
        await self._terminate_terminal(terminal)

    async def _terminal_release(self, params: dict[str, object]) -> None:
        terminal_id = self._terminal_id(params)
        terminal = self._terminals.pop(terminal_id, None)
        if terminal is None:
            raise AcpRequestError(-32602, f"unknown terminalId: {terminal_id}")
        await self._terminate_terminal(terminal)

    async def _terminate_terminal(self, terminal: TerminalSession) -> None:
        if terminal.process.returncode is None:
            terminal.process.terminate()
            try:
                await asyncio.wait_for(terminal.process.wait(), timeout=5.0)
            except TimeoutError:
                terminal.process.kill()
                await terminal.process.wait()
        if terminal.reader_task is not None:
            await terminal.reader_task

    def _workspace_root(self) -> Path:
        return Path(self._workspace_dir).resolve(strict=False)

    def _workspace_path(self, value: object) -> Path:
        if not isinstance(value, str) or not value:
            raise AcpRequestError(-32602, "path must be a non-empty string")
        path = Path(value)
        if not path.is_absolute():
            raise AcpRequestError(-32602, "path must be absolute")
        root = self._workspace_root()
        resolved = path.resolve(strict=False)
        try:
            resolved.relative_to(root)
        except ValueError as exc:
            raise AcpRequestError(
                -32602,
                f"path is outside workspace: {resolved}",
            ) from exc
        return resolved

    def _optional_int(self, value: object, name: str) -> int | None:
        if value is None:
            return None
        if isinstance(value, bool) or not isinstance(value, int):
            raise AcpRequestError(-32602, f"{name} must be a number")
        return value

    def _string_list(self, value: object, name: str) -> list[str]:
        if value is None:
            return []
        if not isinstance(value, list):
            raise AcpRequestError(-32602, f"{name} must be an array")
        result: list[str] = []
        for item in cast("list[object]", value):
            if not isinstance(item, str):
                raise AcpRequestError(-32602, f"{name} entries must be strings")
            result.append(item)
        return result

    def _terminal_env(self, value: object) -> dict[str, str]:
        env = self._build_env()
        for key in (*_COPILOT_SECRET_ENV_VARS, *_CONNECTION_SECRET_ENV_VARS):
            env.pop(key, None)
        if value is None:
            return env
        if not isinstance(value, list):
            raise AcpRequestError(-32602, "env must be an array")
        for item in cast("list[object]", value):
            item_dict = self._as_dict(item)
            name = item_dict.get("name")
            item_value = item_dict.get("value")
            if not isinstance(name, str) or not isinstance(item_value, str):
                raise AcpRequestError(
                    -32602,
                    "env entries must contain string name and value",
                )
            if name in (*_COPILOT_SECRET_ENV_VARS, *_CONNECTION_SECRET_ENV_VARS):
                raise AcpRequestError(
                    -32602,
                    f"terminal environment cannot include provider secret {name}",
                )
            env[name] = item_value
        return env

    def _terminal_id(self, params: dict[str, object]) -> str:
        terminal_id = params.get("terminalId")
        if not isinstance(terminal_id, str) or not terminal_id:
            raise AcpRequestError(-32602, "terminalId must be a non-empty string")
        return terminal_id

    def _terminal(self, params: dict[str, object]) -> TerminalSession:
        terminal_id = self._terminal_id(params)
        terminal = self._terminals.get(terminal_id)
        if terminal is None:
            raise AcpRequestError(-32602, f"unknown terminalId: {terminal_id}")
        return terminal

    def _exit_status(self, returncode: int) -> dict[str, object]:
        if returncode < 0:
            return {"exitCode": None, "signal": str(-returncode)}
        return {"exitCode": returncode, "signal": None}

    def _truncate_terminal_output(self, terminal: TerminalSession) -> None:
        if terminal.output_limit == 0:
            if terminal.output:
                terminal.truncated = True
            terminal.output = ""
            return
        data = terminal.output.encode("utf-8")
        if len(data) <= terminal.output_limit:
            return
        terminal.truncated = True
        terminal.output = data[-terminal.output_limit :].decode(
            "utf-8",
            errors="ignore",
        )

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
        await self._finish_turn(status)
        await self._stop_process()

    async def _finish_turn(self, status: KernelStatus) -> None:
        self._status = status
        if status != KernelStatus.ERROR:
            await self._queue.put(status_event(KernelStatus.DONE))
        else:
            await self._queue.put(status_event(KernelStatus.ERROR))
            await self._queue.put(status_event(KernelStatus.DONE))
        await self._queue.put(session_end())
        await self._queue.put(None)

    async def _stop_process(self) -> None:
        for future in self._pending.values():
            if not future.done():
                future.set_exception(RuntimeError("ACP server stopped"))
        self._pending = {}

        for terminal in list(self._terminals.values()):
            await self._terminate_terminal(terminal)
        self._terminals = {}

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
