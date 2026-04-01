from __future__ import annotations

import logging
import os

import uvicorn


def main() -> None:
    log_level = os.environ.get("LOG_LEVEL", "DEBUG").upper()
    logging.basicConfig(
        level=getattr(logging, log_level, logging.DEBUG),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    uvicorn.run(
        "client_service.app:app",
        host="0.0.0.0",  # noqa: S104
        port=8002,
        reload=False,
        log_level=log_level.lower(),
    )


if __name__ == "__main__":
    main()
