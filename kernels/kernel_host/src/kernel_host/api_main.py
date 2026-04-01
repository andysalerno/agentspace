from __future__ import annotations

import copy
import logging
import os

import uvicorn
from uvicorn.config import LOGGING_CONFIG


def _log_level_from_env() -> int:
    level_name = os.environ.get("LOG_LEVEL", "INFO").upper()
    return getattr(logging, level_name, logging.INFO)


def _uvicorn_log_config(log_level: int) -> dict[str, object]:
    config = copy.deepcopy(LOGGING_CONFIG)
    config["formatters"]["default"]["fmt"] = "%(levelprefix)s %(name)s: %(message)s"
    config["handlers"]["default"]["stream"] = "ext://sys.stdout"
    config["loggers"]["uvicorn"]["level"] = log_level
    config["loggers"]["uvicorn.error"]["level"] = log_level
    config["loggers"]["uvicorn.access"]["level"] = log_level
    config["root"] = {"handlers": ["default"], "level": log_level}
    return config


def main() -> None:
    log_level = _log_level_from_env()
    uvicorn.run(
        "kernel_host.app:app",
        host="0.0.0.0",  # noqa: S104
        port=8000,
        reload=False,
        log_level=log_level,
        log_config=_uvicorn_log_config(log_level),
    )


if __name__ == "__main__":
    main()
