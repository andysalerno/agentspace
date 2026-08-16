from __future__ import annotations

import json
import os
from pathlib import Path
from typing import cast

from kernel_host.terminal import (
    TERMINAL_LAUNCH_ARGV_ENV,
    TERMINAL_LAUNCH_CWD_ENV,
    TerminalConfigurationError,
)


def launch_argv_from_env(env: dict[str, str]) -> tuple[str, ...]:
    raw = env.pop(TERMINAL_LAUNCH_ARGV_ENV, "")
    try:
        value: object = json.loads(raw)
    except json.JSONDecodeError as error:
        msg = "terminal launch argv is missing or invalid JSON"
        raise TerminalConfigurationError(msg) from error
    if not isinstance(value, list):
        msg = "terminal launch argv must be a non-empty JSON string array"
        raise TerminalConfigurationError(msg)
    items = cast("list[object]", value)
    if not items or not all(isinstance(item, str) and item for item in items):
        msg = "terminal launch argv must be a non-empty JSON string array"
        raise TerminalConfigurationError(msg)
    return tuple(cast("list[str]", items))


def launch_cwd_from_env(env: dict[str, str]) -> str:
    cwd = env.pop(TERMINAL_LAUNCH_CWD_ENV, "")
    if not cwd or not Path(cwd).is_absolute():
        msg = "terminal launch cwd must be an absolute path"
        raise TerminalConfigurationError(msg)
    return cwd


def main() -> None:
    environment = dict(os.environ)
    argv = launch_argv_from_env(environment)
    cwd = launch_cwd_from_env(environment)
    os.chdir(cwd)
    os.execvpe(argv[0], argv, environment)  # noqa: S606


if __name__ == "__main__":
    main()
