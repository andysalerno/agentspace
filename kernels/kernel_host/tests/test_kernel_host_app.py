from __future__ import annotations

from typing import Any

import pytest
from kernel_host import app as app_module


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
