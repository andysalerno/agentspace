from __future__ import annotations

from typing import Any

import pytest
from fastapi import HTTPException
from fastapi.routing import APIRoute
from kernel_host import app as app_module
from kernel_host.terminal import (
    AttachKind,
    TerminalClient,
    TerminalClientError,
    TerminalState,
    TerminalStatus,
)


def _set_code_server_found(monkeypatch: pytest.MonkeyPatch) -> None:
    def fake_which(_command: str) -> str:
        return "/usr/bin/code-server"

    monkeypatch.setattr(
        app_module.shutil,
        "which",
        fake_which,
    )


async def _start_vscode_server() -> object:
    start_vscode_server: Any = app_module.__dict__["_start_vscode_server"]
    return await start_vscode_server()


@pytest.mark.asyncio
async def test_vscode_server_defaults_to_workspace_dir(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}
    process = object()

    async def fake_create_subprocess_exec(*args: object) -> object:
        captured["args"] = args
        return process

    monkeypatch.setenv("KERNEL_WORKDIR", "/app")
    monkeypatch.delenv("KERNEL_VSCODE_WORKDIR", raising=False)
    _set_code_server_found(monkeypatch)
    monkeypatch.setattr(
        app_module.asyncio,
        "create_subprocess_exec",
        fake_create_subprocess_exec,
    )

    result = await _start_vscode_server()

    assert result is process
    assert captured["args"][-1] == "/workspace"


@pytest.mark.asyncio
async def test_vscode_server_respects_vscode_workdir_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    async def fake_create_subprocess_exec(*args: object) -> object:
        captured["args"] = args
        return object()

    monkeypatch.setenv("KERNEL_VSCODE_WORKDIR", "/custom-workspace")
    _set_code_server_found(monkeypatch)
    monkeypatch.setattr(
        app_module.asyncio,
        "create_subprocess_exec",
        fake_create_subprocess_exec,
    )

    await _start_vscode_server()

    assert captured["args"][-1] == "/custom-workspace"


@pytest.mark.asyncio
async def test_vscode_server_uses_configured_command(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, str] = {}

    def fake_which(command: str) -> str:
        captured["command"] = command
        return "/usr/bin/custom-code-server"

    async def fake_create_subprocess_exec(*_args: object) -> object:
        return object()

    monkeypatch.setenv("KERNEL_VSCODE_COMMAND", "custom-code-server")
    monkeypatch.setattr(app_module.shutil, "which", fake_which)
    monkeypatch.setattr(
        app_module.asyncio,
        "create_subprocess_exec",
        fake_create_subprocess_exec,
    )

    await _start_vscode_server()

    assert captured["command"] == "custom-code-server"


class StubTerminalController:
    def __init__(self) -> None:
        self.copy_mode_ids: list[str] = []
        self.raise_client_error = False

    async def ensure(self) -> TerminalStatus:
        return _terminal_status(attach_kind=AttachKind.STARTED)

    async def status(self) -> TerminalStatus:
        return _terminal_status()

    async def stop(self) -> TerminalStatus:
        return _terminal_status(state=TerminalState.MISSING)

    async def resume(self) -> TerminalStatus:
        return _terminal_status(attach_kind=AttachKind.RESUMED)

    async def copy_mode(self, tmux_client_id: str) -> TerminalStatus:
        if self.raise_client_error:
            msg = "unknown tmux client"
            raise TerminalClientError(msg)
        self.copy_mode_ids.append(tmux_client_id)
        return _terminal_status()


def _terminal_status(
    *,
    state: TerminalState = TerminalState.RUNNING,
    attach_kind: AttachKind | None = None,
) -> TerminalStatus:
    clients = (
        TerminalClient(
            id="/dev/pts/7",
            tty="/dev/pts/7",
            pid=77,
            width=120,
            height=40,
            session_name="agentspace-test",
            pane_id="%0",
        ),
    )
    return TerminalStatus(
        state=state,
        session_name="agentspace-test",
        target_session="=agentspace-test",
        socket_path="/run/agentspace-tmux.sock",
        attach_argv=("tmux", "attach-session", "-t", "=agentspace-test"),
        pane_id=None if state == TerminalState.MISSING else "%0",
        pane_pid=None if state == TerminalState.MISSING else 88,
        attach_kind=attach_kind,
        clients=() if state == TerminalState.MISSING else clients,
    )


def test_current_and_terminal_routes_are_registered() -> None:
    routes = {
        (method, route.path)
        for route in app_module.app.routes
        if isinstance(route, APIRoute)
        for method in route.methods or set()
    }

    assert {
        ("GET", "/healthz"),
        ("GET", "/session"),
        ("POST", "/messages"),
        ("POST", "/messages/stream"),
        ("GET", "/history"),
        ("GET", "/logs"),
        ("POST", "/reset"),
        ("DELETE", "/session"),
        ("POST", "/terminal/ensure"),
        ("GET", "/terminal"),
        ("POST", "/terminal/stop"),
        ("POST", "/terminal/resume"),
        ("POST", "/terminal/copy-mode"),
    } <= routes


@pytest.mark.asyncio
async def test_terminal_routes_return_structured_metadata(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    controller = StubTerminalController()
    monkeypatch.setattr(app_module, "terminal_controller", controller)

    ensured = await app_module.terminal_ensure()
    observed = await app_module.terminal_status()
    copied = await app_module.terminal_copy_mode(
        app_module.CopyModeRequest(tmux_client_id="/dev/pts/7"),
    )

    assert ensured["state"] == TerminalState.RUNNING
    assert ensured["attach_kind"] == AttachKind.STARTED
    assert observed["attachment_count"] == 1
    assert observed["clients"][0]["id"] == "/dev/pts/7"
    assert copied["pane_id"] == "%0"
    assert controller.copy_mode_ids == ["/dev/pts/7"]

    controller.raise_client_error = True
    with pytest.raises(HTTPException) as error:
        await app_module.terminal_copy_mode(
            app_module.CopyModeRequest(tmux_client_id="/dev/pts/8"),
        )
    assert error.value.status_code == 404
