from __future__ import annotations

import json

import pytest
from kernel_host.terminal import (
    TERMINAL_LAUNCH_ARGV_ENV,
    TERMINAL_LAUNCH_CWD_ENV,
    TerminalConfigurationError,
)
from kernel_host.terminal_process import launch_argv_from_env, launch_cwd_from_env


def test_launch_argv_from_env_returns_exact_tokens_and_removes_payload() -> None:
    argv = ("copilot", ";", "value with spaces", "trailing;")
    env = {
        TERMINAL_LAUNCH_ARGV_ENV: json.dumps(argv),
        "OTHER": "retained",
    }

    parsed = launch_argv_from_env(env)

    assert parsed == argv
    assert TERMINAL_LAUNCH_ARGV_ENV not in env
    assert env["OTHER"] == "retained"


def test_launch_cwd_from_env_returns_absolute_path_and_removes_payload() -> None:
    env = {
        TERMINAL_LAUNCH_CWD_ENV: "/workspace/path;safe",
        "OTHER": "retained",
    }

    cwd = launch_cwd_from_env(env)

    assert cwd == "/workspace/path;safe"
    assert TERMINAL_LAUNCH_CWD_ENV not in env
    assert env["OTHER"] == "retained"


@pytest.mark.parametrize("cwd", ["", "relative/path"])
def test_launch_cwd_from_env_rejects_invalid_path(cwd: str) -> None:
    with pytest.raises(TerminalConfigurationError, match="absolute path"):
        launch_cwd_from_env({TERMINAL_LAUNCH_CWD_ENV: cwd})


@pytest.mark.parametrize(
    "raw",
    [
        "",
        "{}",
        "[]",
        '["copilot", 7]',
        '[""]',
    ],
)
def test_launch_argv_from_env_rejects_invalid_payload(raw: str) -> None:
    with pytest.raises(TerminalConfigurationError, match="terminal launch argv"):
        launch_argv_from_env({TERMINAL_LAUNCH_ARGV_ENV: raw})
