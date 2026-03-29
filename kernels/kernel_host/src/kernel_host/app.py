from __future__ import annotations

from dataclasses import asdict
from typing import TYPE_CHECKING, Any

from fastapi import FastAPI
from pydantic import BaseModel

from kernel_host.service import service_from_env

if TYPE_CHECKING:
    from kernel.events import KernelEvent

app = FastAPI(title="Kernel Host", version="0.1.0")
service = service_from_env()


class SendMessageRequest(BaseModel):
    message: str


def _serialize_events(events: list[KernelEvent]) -> list[dict[str, Any]]:
    return [asdict(event) for event in events]


@app.get("/healthz")
async def healthz() -> dict[str, str]:
    return {"status": "ok"}


@app.get("/session")
async def session_summary() -> dict[str, Any]:
    return await service.summary()


@app.post("/messages")
async def send_message(payload: SendMessageRequest) -> dict[str, Any]:
    events = await service.send_message(payload.message)
    return {"events": _serialize_events(events)}


@app.get("/history")
async def history() -> dict[str, Any]:
    turns = await service.history()
    return {"history": [_serialize_events(events) for events in turns]}


@app.get("/logs")
async def logs() -> dict[str, Any]:
    return {"lines": await service.logs()}


@app.post("/reset")
async def reset() -> dict[str, Any]:
    return await service.reset()


@app.delete("/session", status_code=204)
async def stop() -> None:
    await service.stop()
