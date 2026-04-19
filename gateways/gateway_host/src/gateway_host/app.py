from __future__ import annotations

import logging
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING

from fastapi import FastAPI

from gateway_host.service import service_from_env

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

logger = logging.getLogger(__name__)

service = service_from_env()


@asynccontextmanager
async def lifespan(_app: FastAPI) -> AsyncIterator[None]:
    await service.start()
    try:
        yield
    finally:
        await service.stop()


app = FastAPI(title="Gateway Host", version="0.1.0", lifespan=lifespan)

extra_router = service.gateway.extra_router()
if extra_router is not None:
    app.include_router(extra_router)


@app.get("/healthz")
async def healthz() -> dict[str, str]:
    return {"status": "ok"}


@app.get("/status")
async def status() -> dict[str, object]:
    return service.status_summary()


@app.get("/logs")
async def logs() -> dict[str, object]:
    return {"lines": service.logs()}


@app.delete("/", status_code=204)
async def stop() -> None:
    await service.stop()
