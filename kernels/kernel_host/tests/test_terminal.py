from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, cast

import pytest
from kernel_host.terminal import (
    TERMINAL_LAUNCH_ARGV_ENV,
    TERMINAL_LAUNCH_CWD_ENV,
    AttachKind,
    CommandResult,
    TerminalClientError,
    TerminalCommandError,
    TerminalConfigurationError,
    TerminalController,
    TerminalControllerConfig,
    TerminalState,
    TerminalStateError,
    parse_tmux_version,
    terminal_controller_from_env,
)

if TYPE_CHECKING:
    from collections.abc import Mapping

SESSION_ID = "12345678-1234-5678-9234-567812345678"
RUNTIME_ID = "87654321-4321-8765-a321-876543210000"
ROOT = Path(__file__).parents[1]
TMUX_CONFIG = ROOT / "tmux.conf"


@dataclass(frozen=True, slots=True)
class CommandCall:
    argv: tuple[str, ...]
    env: dict[str, str] | None
    cwd: str | None


class FakeTmuxRunner:
    def __init__(
        self,
        *,
        exists: bool = False,
        dead: bool = False,
        exit_status: int = 0,
    ) -> None:
        self.exists = exists
        self.dead = dead
        self.exit_status = exit_status
        self.pane_pid = 4200
        self.pane_id = "%0"
        self.process_starts = 1 if exists else 0
        self.calls: list[CommandCall] = []
        self.clients: list[tuple[str, str, int, int, int, str, str]] = []
        self.fail_new = False
        self.fail_has_session = False
        self.copy_mode_panes: list[str] = []

    async def run(  # noqa: C901, PLR0911, PLR0912
        self,
        argv: tuple[str, ...],
        *,
        env: Mapping[str, str] | None = None,
        cwd: str | None = None,
    ) -> CommandResult:
        self.calls.append(
            CommandCall(
                argv=argv,
                env=None if env is None else dict(env),
                cwd=cwd,
            ),
        )
        if argv == ("tmux", "-V"):
            return CommandResult(0, "tmux 3.5a\n")

        command = argv[6]
        if command == "has-session":
            if self.fail_has_session:
                return CommandResult(2, stderr="tmux server unavailable")
            return CommandResult(0 if self.exists else 1)
        if command == "new-session":
            if self.fail_new:
                return CommandResult(2, stderr="failed to create session")
            if self.exists:
                return CommandResult(1, stderr="duplicate session")
            self.exists = True
            self.dead = False
            self.exit_status = 0
            self.process_starts += 1
            self.pane_pid += 1
            return CommandResult(0)
        if command == "list-panes":
            if not self.exists:
                return CommandResult(1, stderr="can't find session")
            dead_status = str(self.exit_status) if self.dead else ""
            stdout = (
                f"{int(self.dead)}\t{dead_status}\t{self.pane_pid}\t{self.pane_id}\n"
            )
            return CommandResult(0, stdout)
        if command == "list-clients":
            if not self.exists:
                return CommandResult(1, stderr="can't find session")
            stdout = "".join(
                "\t".join(str(field) for field in client) + "\n"
                for client in self.clients
            )
            return CommandResult(0, stdout)
        if command == "respawn-pane":
            if not self.exists:
                return CommandResult(1, stderr="can't find pane")
            if not self.dead:
                return CommandResult(1, stderr="pane still active")
            self.dead = False
            self.exit_status = 0
            self.process_starts += 1
            self.pane_pid += 1
            return CommandResult(0)
        if command == "kill-session":
            if not self.exists:
                return CommandResult(1, stderr="can't find session")
            self.exists = False
            self.clients.clear()
            return CommandResult(0)
        if command == "copy-mode":
            self.copy_mode_panes.append(argv[-1])
            return CommandResult(0)
        raise AssertionError(argv)

    def command_calls(self, command: str) -> list[CommandCall]:
        return [
            call
            for call in self.calls
            if len(call.argv) > 6 and call.argv[6] == command
        ]


def _controller(
    workspace: Path,
    runner: FakeTmuxRunner,
    *,
    env: dict[str, str] | None = None,
    resume_on_create: bool = False,
    session_name: str | None = None,
) -> TerminalController:
    return TerminalController(
        TerminalControllerConfig(
            runtime_session_id=RUNTIME_ID,
            copilot_session_id=SESSION_ID,
            env={} if env is None else env,
            workspace_dir=str(workspace),
            session_name=session_name,
            tmux_config_path=str(TMUX_CONFIG),
            tmux_socket_path="/run/agentspace-test-tmux.sock",
            resume_on_create=resume_on_create,
        ),
        runner=runner,
    )


def _copilot_argv(call: CommandCall) -> tuple[str, ...]:
    assert call.env is not None
    value: object = json.loads(call.env[TERMINAL_LAUNCH_ARGV_ENV])
    assert isinstance(value, list)
    parsed = cast("list[object]", value)
    assert all(isinstance(item, str) for item in parsed)
    return tuple(cast("list[str]", parsed))


def test_locked_tmux_config_and_image_contract() -> None:
    config = TMUX_CONFIG.read_text(encoding="utf-8")
    dockerfile = (ROOT / "Dockerfile").read_text(encoding="utf-8")

    assert "set-option -g prefix None" in config
    assert "set-option -g prefix2 None" in config
    assert "unbind-key -a -T root" in config
    assert "unbind-key -a -T prefix" in config
    assert "unbind-key -a -T copy-mode" not in config
    assert "unbind-key -a -T copy-mode-vi" not in config
    assert "set-option -g destroy-unattached off" in config
    assert "set-option -g mouse off" in config
    assert "set-window-option -g remain-on-exit on" in config
    assert "set-window-option -g history-limit 100000" in config
    assert "set-window-option -g window-size smallest" in config
    assert "tmux \\" in dockerfile
    assert 'dpkg --compare-versions "$tmux_version" ge "3.2"' in dockerfile
    assert "tmux=" not in dockerfile
    assert "LANG=C.UTF-8" in dockerfile
    assert "LC_ALL=C.UTF-8" in dockerfile
    assert "COPY --chmod=0444 kernels/kernel_host/tmux.conf" in dockerfile


@pytest.mark.parametrize(
    ("output", "raw"),
    [
        ("tmux 3.2", "3.2"),
        ("tmux 3.5a", "3.5a"),
    ],
)
def test_tmux_version_validation(output: str, raw: str) -> None:
    version = parse_tmux_version(output)

    assert version.raw == raw
    assert version.supported


def test_tmux_version_rejects_version_below_minimum() -> None:
    assert not parse_tmux_version("tmux 2.9").supported


def test_tmux_version_rejects_unrecognized_output() -> None:
    with pytest.raises(TerminalConfigurationError, match="unrecognized"):
        parse_tmux_version("not tmux")


@pytest.mark.asyncio
async def test_validate_runtime_logs_supported_tmux_version(tmp_path: Path) -> None:
    controller = _controller(tmp_path, FakeTmuxRunner())

    version = await controller.validate_runtime()

    assert version.raw == "3.5a"


@pytest.mark.asyncio
async def test_first_ensure_prepares_artifacts_and_starts_tmux(tmp_path: Path) -> None:
    staging = tmp_path / "staging"
    (staging / "alpha").mkdir(parents=True)
    workspace = tmp_path / "workspace"
    runner = FakeTmuxRunner()
    controller = _controller(
        workspace,
        runner,
        env={
            "AGENTSPACE_SESSION_ID": RUNTIME_ID,
            "KERNEL_SYSTEM_PROMPT": "Use the session instructions.",
            "KERNEL_SKILLS_STAGING_DIR": str(staging),
            "KERNEL_ENABLED_SKILLS": "alpha",
            "CONNECTION_URL": "https://provider.example/v1",
            "CONNECTION_API_KEY": "secret",
            "CONNECTION_API_FLAVOR": "responses",
        },
    )

    status = await controller.ensure()

    assert status.state == TerminalState.RUNNING
    assert status.attach_kind == AttachKind.STARTED
    assert status.pane_pid == 4201
    assert status.pane_id == "%0"
    assert status.attachment_count == 0
    assert status.attach_argv[-3:] == (
        "attach-session",
        "-t",
        status.target_session,
    )
    profiles = list((workspace / ".github/agents").glob("*.agent.md"))
    assert len(profiles) == 1
    assert "Use the session instructions." in profiles[0].read_text(encoding="utf-8")
    assert (workspace / ".github/skills/alpha").is_symlink()

    create_call = runner.command_calls("new-session")[0]
    copilot_argv = _copilot_argv(create_call)
    assert copilot_argv[-1].startswith("--secret-env-vars=")
    assert f"--session-id={SESSION_ID}" in copilot_argv
    assert any(arg.startswith("--agent=agentspace-") for arg in copilot_argv)
    assert create_call.argv[-2:] == ("-m", "kernel_host.terminal_process")
    assert create_call.env is not None
    assert create_call.env[TERMINAL_LAUNCH_CWD_ENV] == str(workspace)
    assert create_call.env["COPILOT_PROVIDER_BASE_URL"] == (
        "https://provider.example/v1"
    )
    assert create_call.env["COPILOT_PROVIDER_API_KEY"] == "secret"
    assert "CONNECTION_API_KEY" not in create_call.env


@pytest.mark.asyncio
async def test_duplicate_and_concurrent_ensure_adopt_one_process(
    tmp_path: Path,
) -> None:
    runner = FakeTmuxRunner()
    controller = _controller(tmp_path, runner)

    first, second = await asyncio.gather(controller.ensure(), controller.ensure())
    third = await controller.ensure()

    assert runner.process_starts == 1
    assert [first.attach_kind, second.attach_kind] == [
        AttachKind.STARTED,
        AttachKind.ATTACHED,
    ]
    assert third.attach_kind == AttachKind.ATTACHED
    assert len(runner.command_calls("new-session")) == 3


@pytest.mark.asyncio
async def test_ensure_adopts_existing_live_session(tmp_path: Path) -> None:
    runner = FakeTmuxRunner(exists=True)
    controller = _controller(tmp_path, runner)

    status = await controller.ensure()

    assert status.state == TerminalState.RUNNING
    assert status.attach_kind == AttachKind.ATTACHED
    assert runner.process_starts == 1
    assert len(runner.command_calls("new-session")) == 1


@pytest.mark.asyncio
async def test_dead_pane_reports_exit_status_and_is_not_adopted(
    tmp_path: Path,
) -> None:
    runner = FakeTmuxRunner(exists=True, dead=True, exit_status=17)
    controller = _controller(tmp_path, runner)

    observed = await controller.status()
    ensured = await controller.ensure()

    assert observed.state == TerminalState.EXITED
    assert observed.exit_status == 17
    assert observed.pane_pid == 4200
    assert ensured.state == TerminalState.EXITED
    assert ensured.attach_kind is None
    assert runner.process_starts == 1


@pytest.mark.asyncio
async def test_resume_only_dead_pane_and_coalesces_concurrent_calls(
    tmp_path: Path,
) -> None:
    runner = FakeTmuxRunner(exists=True, dead=True, exit_status=9)
    controller = _controller(tmp_path, runner)

    first, second = await asyncio.gather(controller.resume(), controller.resume())

    assert first.state == TerminalState.RUNNING
    assert first.attach_kind == AttachKind.RESUMED
    assert second == first
    assert runner.process_starts == 2
    respawn = runner.command_calls("respawn-pane")
    assert len(respawn) == 1
    assert "-k" not in respawn[0].argv
    assert f"--session-id={SESSION_ID}" in _copilot_argv(respawn[0])

    with pytest.raises(TerminalStateError, match="requires an exited pane"):
        await controller.resume()


@pytest.mark.asyncio
async def test_stop_preserves_workspace_and_next_ensure_is_resumed(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    runner = FakeTmuxRunner()
    controller = _controller(
        workspace,
        runner,
        env={"KERNEL_SYSTEM_PROMPT": "Durable prompt."},
    )
    await controller.ensure()
    profile = next((workspace / ".github/agents").glob("*.agent.md"))

    stopped = await controller.stop()
    resumed = await controller.ensure()

    assert stopped.state == TerminalState.MISSING
    assert profile.exists()
    assert resumed.state == TerminalState.RUNNING
    assert resumed.attach_kind == AttachKind.RESUMED
    assert runner.process_starts == 2
    session_args = [
        arg
        for call in runner.command_calls("new-session")
        for arg in _copilot_argv(call)
        if arg.startswith("--session-id=")
    ]
    assert session_args == [
        f"--session-id={SESSION_ID}",
        f"--session-id={SESSION_ID}",
    ]


@pytest.mark.asyncio
async def test_missing_and_tmux_errors_are_distinct(tmp_path: Path) -> None:
    missing_runner = FakeTmuxRunner()
    missing = await _controller(tmp_path, missing_runner).status()
    assert missing.state == TerminalState.MISSING
    assert missing.pane_pid is None

    create_error_runner = FakeTmuxRunner()
    create_error_runner.fail_new = True
    with pytest.raises(TerminalCommandError, match="failed to create session"):
        await _controller(tmp_path, create_error_runner).ensure()

    status_error_runner = FakeTmuxRunner()
    status_error_runner.fail_has_session = True
    with pytest.raises(TerminalCommandError, match="tmux server unavailable"):
        await _controller(tmp_path, status_error_runner).status()


def test_invalid_terminal_identity_and_session_ids(tmp_path: Path) -> None:
    runner = FakeTmuxRunner()
    with pytest.raises(TerminalConfigurationError, match="invalid tmux session name"):
        _controller(tmp_path, runner, session_name="bad:session")

    with pytest.raises(TerminalConfigurationError, match="durable Copilot UUID"):
        TerminalController(
            TerminalControllerConfig(
                runtime_session_id=RUNTIME_ID,
                copilot_session_id="not-a-uuid",
                env={},
                tmux_config_path=str(TMUX_CONFIG),
            ),
            runner=runner,
        )

    with pytest.raises(TerminalConfigurationError, match="AGENTSPACE_SESSION_ID"):
        terminal_controller_from_env(
            runner=runner,
            env={"KERNEL_SESSION_ID": SESSION_ID},
        )

    with pytest.raises(TerminalConfigurationError, match="does not exist"):
        TerminalController(
            TerminalControllerConfig(
                runtime_session_id=RUNTIME_ID,
                copilot_session_id=SESSION_ID,
                env={},
                tmux_config_path="/missing/tmux.conf",
            ),
            runner=runner,
        )


@pytest.mark.asyncio
async def test_copy_mode_uses_validated_observed_client_mapping(
    tmp_path: Path,
) -> None:
    runner = FakeTmuxRunner(exists=True)
    runner.clients = [
        (
            "/dev/pts/7",
            "/dev/pts/7",
            700,
            120,
            40,
            "agentspace-session",
            "%0",
        ),
    ]
    controller = _controller(tmp_path, runner)

    status = await controller.copy_mode("/dev/pts/7")

    assert status.attachment_count == 1
    assert status.clients[0].id == "/dev/pts/7"
    assert runner.copy_mode_panes == ["%0"]

    with pytest.raises(TerminalClientError, match="invalid tmux client"):
        await controller.copy_mode("-bad")
    with pytest.raises(TerminalClientError, match=r"clients\[\]\.id"):
        await controller.copy_mode("/dev/pts/8")


@pytest.mark.asyncio
async def test_launch_arguments_are_never_combined_into_a_shell_string(
    tmp_path: Path,
) -> None:
    runner = FakeTmuxRunner()
    dangerous_model = "model; echo not-a-command"
    dangerous_extra = "value with spaces; $(touch should-not-run)"
    dangerous_path = "/workspace/path;still-one-argument"
    controller = TerminalController(
        TerminalControllerConfig(
            runtime_session_id="runtime;not-a-command",
            copilot_session_id=SESSION_ID,
            env={
                "COPILOT_MODEL": dangerous_model,
                "COPILOT_EXTRA_ARGS": (
                    f"--custom-option\n;\ntrailing;\n{dangerous_extra}"
                ),
            },
            additional_paths=(dangerous_path,),
            workspace_dir=str(tmp_path),
            tmux_config_path=str(TMUX_CONFIG),
            tmux_socket_path="/run/agentspace-test-tmux.sock",
        ),
        runner=runner,
    )

    await controller.ensure()

    call = runner.command_calls("new-session")[0]
    tmux_argv = call.argv
    copilot_argv = _copilot_argv(call)
    assert dangerous_model in copilot_argv
    assert dangerous_extra in copilot_argv
    assert dangerous_path in copilot_argv
    assert ";" in copilot_argv
    assert "trailing;" in copilot_argv
    assert dangerous_model not in tmux_argv
    assert dangerous_extra not in tmux_argv
    assert ";" not in tmux_argv
    assert "trailing;" not in tmux_argv
    assert dangerous_path not in tmux_argv
    assert tmux_argv[tmux_argv.index("--") + 2 :] == (
        "-m",
        "kernel_host.terminal_process",
    )

    runner.dead = True
    await controller.resume()
    respawn_call = runner.command_calls("respawn-pane")[0]
    assert _copilot_argv(respawn_call) == copilot_argv
    assert dangerous_model not in respawn_call.argv
    assert dangerous_extra not in respawn_call.argv
    assert ";" not in respawn_call.argv
