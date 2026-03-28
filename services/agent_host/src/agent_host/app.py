from __future__ import annotations

from dataclasses import asdict
from typing import TYPE_CHECKING, Any

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

from agent_host.service import AgentHost, SessionNotFoundError

if TYPE_CHECKING:
    from kernel.events import KernelEvent

app = FastAPI(title="Agent Host", version="0.1.0")
host = AgentHost()


class CreateSessionRequest(BaseModel):
    harness: str = "copilot-cli"
    env: dict[str, str] = Field(default_factory=dict)
    cwd: str | None = None
    additional_paths: list[str] = Field(default_factory=list)


class SendMessageRequest(BaseModel):
    message: str


def _serialize_events(events: list[KernelEvent]) -> list[dict[str, Any]]:
    return [asdict(event) for event in events]


@app.get("/healthz")
async def healthz() -> dict[str, str]:
    return {"status": "ok"}


@app.post("/sessions")
async def create_session(payload: CreateSessionRequest) -> dict[str, Any]:
    return await host.create_session(
        harness=payload.harness,
        env=payload.env,
        cwd=payload.cwd,
        additional_paths=tuple(payload.additional_paths),
    )


@app.get("/sessions")
async def list_sessions() -> list[dict[str, Any]]:
    return await host.list_sessions()


@app.get("/sessions/{session_id}")
async def get_session(session_id: str) -> dict[str, Any]:
    try:
        return await host.get_session(session_id)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.post("/sessions/{session_id}/messages")
async def send_message(
    session_id: str,
    payload: SendMessageRequest,
) -> dict[str, Any]:
    try:
        events = await host.send_message(session_id, payload.message)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {"events": _serialize_events(events)}


@app.get("/sessions/{session_id}/history")
async def history(session_id: str) -> dict[str, Any]:
    try:
        turns = await host.history(session_id)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {"history": [_serialize_events(events) for events in turns]}


@app.post("/sessions/{session_id}/reset")
async def reset_session(session_id: str) -> dict[str, Any]:
    try:
        return await host.reset_session(session_id)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.delete("/sessions/{session_id}", status_code=204)
async def destroy_session(session_id: str) -> None:
    try:
        await host.destroy_session(session_id)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
