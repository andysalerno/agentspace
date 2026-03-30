from __future__ import annotations

import logging
from contextlib import asynccontextmanager
from dataclasses import asdict
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

from fastapi import FastAPI, HTTPException
from kernel_host.registry import HarnessName
from pydantic import BaseModel, Field

from agent_host.service import AgentHost, SessionNotFoundError
from agent_host.skills import (
    BuiltinSkillReadOnlyError,
    InvalidSkillFilePathError,
    InvalidSkillIdError,
    SkillAlreadyExistsError,
    SkillNotFoundError,
    SkillsService,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from kernel.events import KernelEvent

logger = logging.getLogger(__name__)

host = AgentHost()
skills = SkillsService()


@asynccontextmanager
async def lifespan(_app: FastAPI) -> AsyncIterator[None]:
    skills.sync_builtin_skills()
    yield
    await host.destroy_all_sessions()


app = FastAPI(title="Agent Host", version="0.1.0", lifespan=lifespan)


class CreateSessionRequest(BaseModel):
    harness: HarnessName = HarnessName.COPILOT_CLI
    env: dict[str, str] = Field(default_factory=dict)
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


@app.get("/sessions/{session_id}/logs")
async def session_logs(session_id: str) -> dict[str, Any]:
    try:
        lines = await host.logs(session_id)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {"lines": lines}


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


# --- Skills ---


class CreateSkillRequest(BaseModel):
    skill_id: str = Field(pattern=r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
    files: dict[str, str]


class UpdateSkillRequest(BaseModel):
    files: dict[str, str]


@app.post("/skills")
async def create_skill(payload: CreateSkillRequest) -> dict[str, Any]:
    try:
        return skills.create_skill(payload.skill_id, payload.files)
    except SkillAlreadyExistsError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    except (InvalidSkillIdError, InvalidSkillFilePathError) as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@app.get("/skills")
async def list_skills() -> list[dict[str, Any]]:
    return skills.list_skills()


@app.get("/skills/{skill_id}")
async def get_skill(skill_id: str) -> dict[str, Any]:
    try:
        return skills.get_skill(skill_id)
    except SkillNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except InvalidSkillIdError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@app.put("/skills/{skill_id}")
async def update_skill(skill_id: str, payload: UpdateSkillRequest) -> dict[str, Any]:
    try:
        return skills.update_skill(skill_id, payload.files)
    except BuiltinSkillReadOnlyError as exc:
        raise HTTPException(status_code=403, detail=str(exc)) from exc
    except SkillNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except (InvalidSkillIdError, InvalidSkillFilePathError) as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@app.delete("/skills/{skill_id}", status_code=204)
async def delete_skill(skill_id: str) -> None:
    try:
        skills.delete_skill(skill_id)
    except BuiltinSkillReadOnlyError as exc:
        raise HTTPException(status_code=403, detail=str(exc)) from exc
    except SkillNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except InvalidSkillIdError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
