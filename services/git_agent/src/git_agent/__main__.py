from __future__ import annotations

import logging
import os

import uvicorn


def main() -> None:
    log_level = os.environ.get("LOG_LEVEL", "INFO").upper()
    logging.basicConfig(
        level=getattr(logging, log_level, logging.INFO),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    uvicorn.run(
        "git_agent.app:app",
        host="0.0.0.0",  # noqa: S104
        port=8004,
        reload=False,
        log_level=log_level.lower(),
    )


if __name__ == "__main__":
    main()
