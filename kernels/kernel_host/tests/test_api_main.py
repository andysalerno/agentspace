from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any

from kernel_host import api_main

if TYPE_CHECKING:
    import pytest


def test_main_defaults_to_debug_logs_on_stdout(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_run(*args: object, **kwargs: object) -> None:
        captured["args"] = args
        captured["kwargs"] = kwargs

    monkeypatch.delenv("LOG_LEVEL", raising=False)
    monkeypatch.setattr(api_main.uvicorn, "run", fake_run)

    api_main.main()

    kwargs = captured["kwargs"]
    assert kwargs["log_level"] == logging.DEBUG
    assert kwargs["log_config"]["handlers"]["default"]["stream"] == "ext://sys.stdout"
    assert kwargs["log_config"]["root"] == {
        "handlers": ["default"],
        "level": logging.DEBUG,
    }


def test_main_respects_log_level_override(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: dict[str, Any] = {}

    def fake_run(*args: object, **kwargs: object) -> None:
        captured["args"] = args
        captured["kwargs"] = kwargs

    monkeypatch.setenv("LOG_LEVEL", "INFO")
    monkeypatch.setattr(api_main.uvicorn, "run", fake_run)

    api_main.main()

    assert captured["kwargs"]["log_level"] == logging.INFO
