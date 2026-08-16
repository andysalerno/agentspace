from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import os
import re
import sys
import uuid
from dataclasses import dataclass, replace
from enum import StrEnum
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING, Protocol

from copilot_launch import CopilotLaunch, CopilotLaunchConfig, build_interactive_launch

if TYPE_CHECKING:
    from collections.abc import Mapping

logger = logging.getLogger(__name__)

MINIMUM_TMUX_VERSION = (3, 2)
TMUX_CONFIG_PATH = "/etc/agentspace/tmux.conf"
TMUX_SOCKET_PATH = "/run/agentspace-tmux.sock"
TELEMETRY_DIR = "/var/lib/agentspace/telemetry"
TERMINAL_LAUNCH_ARGV_ENV = "AGENTSPACE_TERMINAL_LAUNCH_ARGV"
TERMINAL_LAUNCH_CWD_ENV = "AGENTSPACE_TERMINAL_LAUNCH_CWD"
TERMINAL_ATTACHMENT_ID_ENV = "AGENTSPACE_TERMINAL_ATTACHMENT_ID"
_TMUX_PANE_FORMAT = "#{pane_dead}\t#{pane_dead_status}\t#{pane_pid}\t#{pane_id}"
_TMUX_CLIENT_FORMAT = (
    "#{client_name}\t#{client_tty}\t#{client_pid}\t"
    "#{client_width}\t#{client_height}\t#{session_name}\t#{pane_id}"
)
_SESSION_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}")
_CLIENT_ID = re.compile(r"[A-Za-z0-9_./:@+-]{1,256}")
_PANE_ID = re.compile(r"%[0-9]+")
_TMUX_VERSION = re.compile(r"tmux\s+(\d+)\.(\d+)([a-z]?)")


class TerminalState(StrEnum):
    MISSING = "missing"
    RUNNING = "running"
    EXITED = "exited"


class AttachKind(StrEnum):
    STARTED = "started"
    ATTACHED = "attached"
    RESUMED = "resumed"


class TerminalError(RuntimeError):
    """Base error for terminal controller failures."""


class TerminalConfigurationError(TerminalError):
    """Terminal configuration is missing or unsafe."""


class TerminalCommandError(TerminalError):
    """A tmux command failed unexpectedly."""


class TerminalStateError(TerminalError):
    """The requested operation is invalid for the observed terminal state."""


class TerminalClientError(TerminalError):
    """The requested tmux client is invalid or is not attached."""


@dataclass(frozen=True, slots=True)
class CommandResult:
    returncode: int
    stdout: str = ""
    stderr: str = ""


class CommandRunner(Protocol):
    async def run(
        self,
        argv: tuple[str, ...],
        *,
        env: Mapping[str, str] | None = None,
        cwd: str | None = None,
    ) -> CommandResult: ...


class AsyncCommandRunner:
    async def run(
        self,
        argv: tuple[str, ...],
        *,
        env: Mapping[str, str] | None = None,
        cwd: str | None = None,
    ) -> CommandResult:
        process = await asyncio.create_subprocess_exec(
            *argv,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=None if env is None else dict(env),
            cwd=cwd,
        )
        stdout, stderr = await process.communicate()
        return CommandResult(
            returncode=process.returncode or 0,
            stdout=stdout.decode(errors="replace"),
            stderr=stderr.decode(errors="replace"),
        )


@dataclass(frozen=True, slots=True)
class TerminalClient:
    id: str
    tty: str
    pid: int
    width: int
    height: int
    session_name: str
    pane_id: str
    attachment_id: str | None = None


@dataclass(frozen=True, slots=True)
class TerminalTelemetryLaunch:
    launch_id: str
    file_path: str


@dataclass(frozen=True, slots=True)
class TerminalStatus:
    state: TerminalState
    session_name: str
    target_session: str
    socket_path: str
    attach_argv: tuple[str, ...]
    pane_id: str | None = None
    pane_pid: int | None = None
    exit_status: int | None = None
    attach_kind: AttachKind | None = None
    clients: tuple[TerminalClient, ...] = ()

    @property
    def attachment_count(self) -> int:
        return len(self.clients)


@dataclass(frozen=True, slots=True)
class TerminalControllerConfig:
    runtime_session_id: str
    copilot_session_id: str
    env: Mapping[str, str]
    additional_paths: tuple[str, ...] = ()
    workspace_dir: str = "/workspace"
    session_name: str | None = None
    tmux_executable: str = "tmux"
    tmux_config_path: str = TMUX_CONFIG_PATH
    tmux_socket_path: str = TMUX_SOCKET_PATH
    proc_root: str = "/proc"
    resume_on_create: bool = False

    def resolved_session_name(self) -> str:
        if self.session_name is not None:
            _validate_session_name(self.session_name)
            return self.session_name
        if not self.runtime_session_id:
            msg = "AGENTSPACE_SESSION_ID is required for terminal identity"
            raise TerminalConfigurationError(msg)
        digest = hashlib.sha256(self.runtime_session_id.encode()).hexdigest()[:24]
        return f"agentspace-{digest}"


@dataclass(frozen=True, slots=True)
class TmuxVersion:
    raw: str
    major: int
    minor: int
    suffix: str

    @property
    def supported(self) -> bool:
        return (self.major, self.minor) >= MINIMUM_TMUX_VERSION


class TerminalController:
    def __init__(
        self,
        config: TerminalControllerConfig,
        *,
        runner: CommandRunner | None = None,
    ) -> None:
        self._config = config
        self._runner = runner or AsyncCommandRunner()
        self._session_name = config.resolved_session_name()
        self._target_session = f"={self._session_name}"
        self._ensure_lock = asyncio.Lock()
        self._resume_task: asyncio.Task[TerminalStatus] | None = None
        self._resume_on_create = config.resume_on_create
        self._validate_config()

    @property
    def session_name(self) -> str:
        return self._session_name

    async def validate_runtime(self) -> TmuxVersion:
        try:
            result = await self._runner.run((self._config.tmux_executable, "-V"))
        except OSError as error:
            msg = f"failed to run tmux version check: {error}"
            raise TerminalCommandError(msg) from error
        if result.returncode != 0:
            error = self._command_error("tmux version check", result)
            raise error
        version = parse_tmux_version(result.stdout.strip())
        if not version.supported:
            minimum = ".".join(str(part) for part in MINIMUM_TMUX_VERSION)
            msg = f"tmux {version.raw} is unsupported; minimum version is {minimum}"
            raise TerminalConfigurationError(msg)
        logger.info("terminal controller using tmux %s", version.raw)
        return version

    async def ensure(self) -> TerminalStatus:
        async with self._ensure_lock:
            launch = await self._prepare_launch()
            result = await self._run(
                self._new_session_argv(),
                env=self._tmux_environment(launch),
            )
            if result.returncode == 0:
                kind = (
                    AttachKind.RESUMED if self._resume_on_create else AttachKind.STARTED
                )
                self._resume_on_create = False
                status = await self._status()
                if status.state == TerminalState.MISSING:
                    msg = "tmux reported session creation success but no session exists"
                    raise TerminalCommandError(msg)
                logger.info(
                    "terminal tmux session %s: %s",
                    kind.value,
                    self._session_name,
                )
                return replace(status, attach_kind=kind)

            status = await self._status()
            if status.state == TerminalState.RUNNING:
                logger.info("adopted live terminal tmux session %s", self._session_name)
                return replace(status, attach_kind=AttachKind.ATTACHED)
            if status.state == TerminalState.EXITED:
                return status
            error = self._command_error("create tmux session", result)
            raise error

    async def status(self) -> TerminalStatus:
        return await self._status()

    async def stop(self) -> TerminalStatus:
        status = await self._status()
        if status.state == TerminalState.MISSING:
            return status
        result = await self._run(
            (*self._tmux_argv(), "kill-session", "-t", self._target_session),
        )
        observed = await self._status()
        if result.returncode != 0 and observed.state != TerminalState.MISSING:
            error = self._command_error("stop tmux session", result)
            raise error
        self._resume_on_create = True
        logger.info("stopped terminal tmux session %s", self._session_name)
        return observed

    async def resume(self) -> TerminalStatus:
        task = self._resume_task
        if task is None or task.done():
            task = asyncio.create_task(self._resume_once())
            self._resume_task = task
            task.add_done_callback(self._resume_finished)
        return await asyncio.shield(task)

    async def detach_client(self, tmux_client_id: str) -> TerminalStatus:
        status = await self._status()
        if status.state == TerminalState.MISSING:
            return status
        client = self._observed_client(status, tmux_client_id)
        result = await self._run(
            (*self._tmux_argv(), "detach-client", "-t", client.id),
        )
        observed = await self._status()
        if result.returncode != 0 and any(
            candidate.id == client.id for candidate in observed.clients
        ):
            error = self._command_error("detach tmux client", result)
            raise error
        return observed

    async def _resume_once(self) -> TerminalStatus:
        status = await self._status()
        if status.state != TerminalState.EXITED or status.pane_id is None:
            msg = f"resume requires an exited pane; observed {status.state.value}"
            raise TerminalStateError(msg)

        launch = await self._prepare_launch()
        result = await self._run(
            (
                *self._tmux_argv(),
                "respawn-pane",
                "-t",
                status.pane_id,
                "--",
                *self._terminal_process_argv(),
            ),
            env=self._tmux_environment(launch),
        )
        observed = await self._status()
        if observed.state == TerminalState.RUNNING:
            self._resume_on_create = False
            logger.info("resumed terminal tmux pane %s", status.pane_id)
            return replace(observed, attach_kind=AttachKind.RESUMED)
        if result.returncode != 0:
            error = self._command_error("respawn tmux pane", result)
            raise error
        msg = "tmux reported pane respawn success but the pane is not running"
        raise TerminalCommandError(msg)

    async def _prepare_launch(self) -> CopilotLaunch:
        def prepare() -> CopilotLaunch:
            Path(self._config.workspace_dir).mkdir(parents=True, exist_ok=True)
            launch_id = uuid.uuid4()
            return build_interactive_launch(
                CopilotLaunchConfig(
                    session_id=self._config.copilot_session_id,
                    env={
                        **self._config.env,
                        "AGENTSPACE_SESSION_ID": self._config.runtime_session_id,
                    },
                    additional_paths=self._config.additional_paths,
                    workspace_dir=self._config.workspace_dir,
                ),
                telemetry_file_path=f"{TELEMETRY_DIR}/{launch_id}.jsonl",
            )

        return await asyncio.to_thread(prepare)

    async def _status(self) -> TerminalStatus:
        has_session = await self._run(
            (*self._tmux_argv(), "has-session", "-t", self._target_session),
        )
        if has_session.returncode == 1:
            return self._missing_status()
        if has_session.returncode != 0:
            error = self._command_error("inspect tmux session", has_session)
            raise error

        panes = await self._run(
            (
                *self._tmux_argv(),
                "list-panes",
                "-t",
                self._target_session,
                "-F",
                _TMUX_PANE_FORMAT,
            ),
        )
        if panes.returncode != 0:
            return await self._missing_or_error("inspect tmux pane", panes)
        pane_lines = [line for line in panes.stdout.splitlines() if line]
        if len(pane_lines) != 1:
            msg = (
                f"expected exactly one tmux pane for {self._session_name}, "
                f"observed {len(pane_lines)}"
            )
            raise TerminalCommandError(msg)
        pane_dead, dead_status, pane_pid, pane_id = _parse_pane(pane_lines[0])

        clients_result = await self._run(
            (
                *self._tmux_argv(),
                "list-clients",
                "-t",
                self._target_session,
                "-F",
                _TMUX_CLIENT_FORMAT,
            ),
        )
        if clients_result.returncode != 0:
            return await self._missing_or_error(
                "inspect tmux clients",
                clients_result,
            )
        clients = tuple(
            replace(
                client,
                attachment_id=self._attachment_id_for_pid(client.pid),
            )
            for line in clients_result.stdout.splitlines()
            if line
            for client in (_parse_client(line),)
        )
        state = TerminalState.EXITED if pane_dead else TerminalState.RUNNING
        return TerminalStatus(
            state=state,
            session_name=self._session_name,
            target_session=self._target_session,
            socket_path=self._config.tmux_socket_path,
            attach_argv=self._attach_argv(),
            pane_id=pane_id,
            pane_pid=pane_pid,
            exit_status=dead_status if pane_dead else None,
            clients=clients,
        )

    async def _missing_or_error(
        self,
        operation: str,
        result: CommandResult,
    ) -> TerminalStatus:
        has_session = await self._run(
            (*self._tmux_argv(), "has-session", "-t", self._target_session),
        )
        if has_session.returncode == 1:
            return self._missing_status()
        error = self._command_error(operation, result)
        raise error

    async def _run(
        self,
        argv: tuple[str, ...],
        *,
        env: Mapping[str, str] | None = None,
    ) -> CommandResult:
        try:
            return await self._runner.run(argv, env=env)
        except OSError as error:
            msg = f"failed to execute tmux: {error}"
            raise TerminalCommandError(msg) from error

    def _new_session_argv(self) -> tuple[str, ...]:
        return (
            *self._tmux_argv(),
            "new-session",
            "-d",
            "-s",
            self._session_name,
            "--",
            *self._terminal_process_argv(),
        )

    @staticmethod
    def _tmux_environment(launch: CopilotLaunch) -> dict[str, str]:
        environment = dict(launch.environment)
        environment[TERMINAL_LAUNCH_ARGV_ENV] = json.dumps(
            launch.argv,
            ensure_ascii=False,
            separators=(",", ":"),
        )
        environment[TERMINAL_LAUNCH_CWD_ENV] = launch.cwd
        return environment

    def _tmux_argv(self) -> tuple[str, ...]:
        return (
            self._config.tmux_executable,
            "-u",
            "-S",
            self._config.tmux_socket_path,
            "-f",
            self._config.tmux_config_path,
        )

    @staticmethod
    def _terminal_process_argv() -> tuple[str, ...]:
        return (sys.executable, "-m", "kernel_host.terminal_process")

    def _resume_finished(self, task: asyncio.Task[TerminalStatus]) -> None:
        if not task.cancelled():
            task.exception()
        if self._resume_task is task:
            self._resume_task = None

    def _attach_argv(self) -> tuple[str, ...]:
        return (
            *self._tmux_argv(),
            "attach-session",
            "-t",
            self._target_session,
        )

    def _missing_status(self) -> TerminalStatus:
        return TerminalStatus(
            state=TerminalState.MISSING,
            session_name=self._session_name,
            target_session=self._target_session,
            socket_path=self._config.tmux_socket_path,
            attach_argv=self._attach_argv(),
        )

    def _attachment_id_for_pid(self, pid: int) -> str | None:
        values = self._environment_values_for_pid(pid, TERMINAL_ATTACHMENT_ID_ENV)
        if len(values) != 1:
            return None
        try:
            attachment_id = values[0].decode("ascii")
            parsed = uuid.UUID(attachment_id)
        except (UnicodeDecodeError, ValueError):
            return None
        return attachment_id if str(parsed) == attachment_id else None

    def telemetry_launch_for_pane(
        self, pane_pid: int
    ) -> TerminalTelemetryLaunch | None:
        values = self._environment_values_for_pid(
            pane_pid,
            "COPILOT_OTEL_FILE_EXPORTER_PATH",
        )
        if len(values) != 1:
            return None
        try:
            file_path = values[0].decode("utf-8")
        except UnicodeDecodeError:
            return None
        path = PurePosixPath(file_path)
        if (
            not path.is_absolute()
            or str(path.parent) != TELEMETRY_DIR
            or path.suffix != ".jsonl"
        ):
            return None
        try:
            parsed = uuid.UUID(path.stem)
        except ValueError:
            return None
        launch_id = str(parsed)
        if path.name != f"{launch_id}.jsonl":
            return None
        return TerminalTelemetryLaunch(launch_id=launch_id, file_path=str(path))

    def _environment_values_for_pid(
        self,
        pid: int,
        variable_name: str,
    ) -> list[bytes]:
        try:
            environment = (
                Path(self._config.proc_root) / str(pid) / "environ"
            ).read_bytes()
        except OSError:
            return []
        prefix = f"{variable_name}=".encode()
        return [
            entry.removeprefix(prefix)
            for entry in environment.split(b"\0")
            if entry.startswith(prefix)
        ]

    @staticmethod
    def _observed_client(
        status: TerminalStatus,
        tmux_client_id: str,
    ) -> TerminalClient:
        _validate_client_id(tmux_client_id)
        client = next(
            (
                candidate
                for candidate in status.clients
                if candidate.id == tmux_client_id
            ),
            None,
        )
        if client is None:
            msg = (
                "tmux_client_id must be an observed clients[].id value from "
                "GET /terminal"
            )
            raise TerminalClientError(msg)
        return client

    def _validate_config(self) -> None:
        try:
            uuid.UUID(self._config.copilot_session_id)
        except ValueError as error:
            msg = (
                "KERNEL_SESSION_ID must be the durable Copilot UUID: "
                f"{self._config.copilot_session_id!r}"
            )
            raise TerminalConfigurationError(msg) from error
        if not self._config.tmux_config_path.startswith("/"):
            msg = "tmux config path must be absolute"
            raise TerminalConfigurationError(msg)
        if not Path(self._config.tmux_config_path).is_file():
            msg = f"tmux config file does not exist: {self._config.tmux_config_path}"
            raise TerminalConfigurationError(msg)
        if (
            not self._config.tmux_socket_path.startswith("/")
            or len(self._config.tmux_socket_path.encode()) > 100
        ):
            msg = (
                "tmux socket path must be an absolute Unix socket path under 101 bytes"
            )
            raise TerminalConfigurationError(msg)
        if not Path(self._config.proc_root).is_absolute():
            msg = "proc root must be an absolute path"
            raise TerminalConfigurationError(msg)

    @staticmethod
    def _command_error(operation: str, result: CommandResult) -> TerminalCommandError:
        detail = result.stderr.strip() or result.stdout.strip() or "no command output"
        return TerminalCommandError(
            f"{operation} failed with exit code {result.returncode}: {detail}",
        )


def parse_tmux_version(output: str) -> TmuxVersion:
    match = _TMUX_VERSION.fullmatch(output.strip())
    if match is None:
        msg = f"unrecognized tmux version output: {output!r}"
        raise TerminalConfigurationError(msg)
    major, minor, suffix = match.groups()
    return TmuxVersion(
        raw=f"{major}.{minor}{suffix}",
        major=int(major),
        minor=int(minor),
        suffix=suffix,
    )


def terminal_controller_from_env(
    *,
    runner: CommandRunner | None = None,
    env: Mapping[str, str] | None = None,
) -> TerminalController:
    source = dict(os.environ if env is None else env)
    runtime_session_id = source.get("AGENTSPACE_SESSION_ID", "")
    copilot_session_id = source.get("KERNEL_SESSION_ID", "")
    additional_paths = tuple(
        path
        for path in source.get("KERNEL_ADDITIONAL_PATHS", "").split(os.pathsep)
        if path
    )
    resume_on_create = source.get("KERNEL_TERMINAL_RESUME", "").lower() in {
        "1",
        "true",
        "yes",
        "on",
    }
    return TerminalController(
        TerminalControllerConfig(
            runtime_session_id=runtime_session_id,
            copilot_session_id=copilot_session_id,
            env=source,
            additional_paths=additional_paths,
            workspace_dir=source.get("COPILOT_WORKSPACE_DIR", "/workspace"),
            resume_on_create=resume_on_create,
        ),
        runner=runner,
    )


def _validate_session_name(session_name: str) -> None:
    if _SESSION_NAME.fullmatch(session_name) is None:
        msg = f"invalid tmux session name: {session_name!r}"
        raise TerminalConfigurationError(msg)


def _validate_client_id(client_id: str) -> None:
    if client_id.startswith("-") or _CLIENT_ID.fullmatch(client_id) is None:
        msg = f"invalid tmux client identifier: {client_id!r}"
        raise TerminalClientError(msg)


def _parse_pane(line: str) -> tuple[bool, int | None, int, str]:
    fields = line.split("\t")
    if len(fields) != 4:
        msg = f"invalid tmux pane status: {line!r}"
        raise TerminalCommandError(msg)
    dead_raw, status_raw, pid_raw, pane_id = fields
    if dead_raw not in {"0", "1"} or _PANE_ID.fullmatch(pane_id) is None:
        msg = f"invalid tmux pane status: {line!r}"
        raise TerminalCommandError(msg)
    try:
        pane_pid = int(pid_raw)
        dead_status = int(status_raw) if dead_raw == "1" else None
    except ValueError as error:
        msg = f"invalid tmux pane status: {line!r}"
        raise TerminalCommandError(msg) from error
    return dead_raw == "1", dead_status, pane_pid, pane_id


def _parse_client(line: str) -> TerminalClient:
    fields = line.split("\t")
    if len(fields) != 7:
        msg = f"invalid tmux client status: {line!r}"
        raise TerminalCommandError(msg)
    client_id, tty, pid_raw, width_raw, height_raw, session_name, pane_id = fields
    _validate_client_id(client_id)
    if _PANE_ID.fullmatch(pane_id) is None:
        msg = f"invalid tmux client pane identifier: {pane_id!r}"
        raise TerminalCommandError(msg)
    try:
        pid = int(pid_raw)
        width = int(width_raw)
        height = int(height_raw)
    except ValueError as error:
        msg = f"invalid tmux client status: {line!r}"
        raise TerminalCommandError(msg) from error
    return TerminalClient(
        id=client_id,
        tty=tty,
        pid=pid,
        width=width,
        height=height,
        session_name=session_name,
        pane_id=pane_id,
    )
