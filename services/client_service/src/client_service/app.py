from __future__ import annotations

import json
import logging
from typing import TYPE_CHECKING, Any

import httpx
from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse
from kernel_host.registry import HarnessName
from pydantic import BaseModel, Field

from client_service.models import ClientType  # noqa: TC001
from client_service.service import (
    AgentAlreadyExistsError,
    AgentNotFoundError,
    ClientService,
    InvalidAgentIdError,
    KernelNotFoundError,
    SessionNotFoundError,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

logging.basicConfig(level=logging.INFO)

app = FastAPI(title="Client Service", version="0.1.0")
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=False,
    allow_methods=["*"],
    allow_headers=["*"],
)
service = ClientService()


class CreateAgentRequest(BaseModel):
    agent_id: str = Field(pattern=r"^[a-z]+(?:-[a-z]+)*$")
    name: str
    harness: HarnessName = HarnessName.COPILOT_CLI
    system_prompt: str = ""
    skills: list[str] = Field(default_factory=list)


class UpdateAgentRequest(BaseModel):
    name: str | None = None
    harness: HarnessName | None = None
    system_prompt: str | None = None
    skills: list[str] | None = None


class CreateSessionRequest(BaseModel):
    agent_id: str
    channel_name: str | None = None
    client_type: ClientType | None = None


class SendMessageRequest(BaseModel):
    message: str


@app.get("/healthz")
async def healthz() -> dict[str, str]:
    return {"status": "ok"}


@app.post("/agents")
async def create_agent(payload: CreateAgentRequest) -> dict[str, object]:
    try:
        return await service.create_agent(
            agent_id=payload.agent_id,
            name=payload.name,
            harness=payload.harness,
            system_prompt=payload.system_prompt,
            skills=payload.skills,
        )
    except AgentAlreadyExistsError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    except InvalidAgentIdError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc


@app.get("/agents")
async def list_agents() -> list[dict[str, object]]:
    return await service.list_agents()


@app.get("/agents/{agent_id}")
async def get_agent(agent_id: str) -> dict[str, object]:
    try:
        return await service.get_agent(agent_id)
    except AgentNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.patch("/agents/{agent_id}")
async def update_agent(
    agent_id: str,
    payload: UpdateAgentRequest,
) -> dict[str, object]:
    try:
        return await service.update_agent(
            agent_id,
            name=payload.name,
            harness=payload.harness,
            system_prompt=payload.system_prompt,
            skills=payload.skills,
        )
    except AgentNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.delete("/agents/{agent_id}", status_code=204)
async def delete_agent(agent_id: str) -> None:
    try:
        await service.delete_agent(agent_id)
    except AgentNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.post("/sessions")
async def create_session(payload: CreateSessionRequest) -> dict[str, object]:
    try:
        return await service.create_session(
            agent_id=payload.agent_id,
            channel_name=payload.channel_name,
            client_type=payload.client_type,
        )
    except AgentNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.get("/sessions")
async def list_sessions() -> list[dict[str, object]]:
    return await service.list_sessions()


@app.get("/sessions/{session_id}")
async def get_session(session_id: str) -> dict[str, object]:
    try:
        return await service.get_session(session_id)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.get("/sessions/{session_id}/messages")
async def list_messages(session_id: str) -> dict[str, list[dict[str, object]]]:
    try:
        return {"messages": await service.list_messages(session_id)}
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.post("/sessions/{session_id}/messages")
async def send_message(
    session_id: str,
    payload: SendMessageRequest,
) -> dict[str, Any]:
    try:
        return await service.send_message(session_id, payload.message)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.post("/sessions/{session_id}/messages/stream")
async def stream_message(
    session_id: str,
    payload: SendMessageRequest,
) -> StreamingResponse:
    try:
        stream = service.stream_message(session_id, payload.message)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc

    async def response_lines() -> AsyncIterator[str]:
        async for item in stream:
            yield json.dumps(item, separators=(",", ":")) + "\n"

    return StreamingResponse(
        response_lines(),
        media_type="application/x-ndjson",
        headers={
            "Cache-Control": "no-cache",
            "X-Accel-Buffering": "no",
        },
    )


@app.post("/sessions/{session_id}/reset")
async def reset_session(session_id: str) -> dict[str, object]:
    try:
        return await service.reset_session(session_id)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.delete("/sessions/{session_id}", status_code=204)
async def delete_session(session_id: str) -> None:
    try:
        await service.delete_session(session_id)
    except SessionNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.get("/kernels")
async def list_kernels() -> list[dict[str, object]]:
    return await service.list_kernels()


@app.delete("/kernels/{kernel_session_id}", status_code=204)
async def kill_kernel(kernel_session_id: str) -> None:
    try:
        await service.kill_kernel(kernel_session_id)
    except KernelNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.get("/kernels/{kernel_session_id}/logs")
async def kernel_logs(kernel_session_id: str) -> dict[str, Any]:
    try:
        lines = await service.kernel_logs(kernel_session_id)
    except KernelNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {"lines": lines}


# --- Skills ---


class CreateSkillRequest(BaseModel):
    skill_id: str = Field(pattern=r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
    files: dict[str, str]


class UpdateSkillRequest(BaseModel):
    files: dict[str, str]


@app.post("/skills")
async def create_skill(payload: CreateSkillRequest) -> dict[str, Any]:
    try:
        return await service.create_skill(payload.skill_id, payload.files)
    except HTTPException:
        raise
    except Exception as exc:
        status = _status_for_skill_error(exc)
        raise HTTPException(status_code=status, detail=str(exc)) from exc


@app.get("/skills")
async def list_skills() -> list[dict[str, Any]]:
    return await service.list_skills()


@app.get("/skills/{skill_id}")
async def get_skill(skill_id: str) -> dict[str, Any]:
    try:
        return await service.get_skill(skill_id)
    except HTTPException:
        raise
    except Exception as exc:
        status = _status_for_skill_error(exc)
        raise HTTPException(status_code=status, detail=str(exc)) from exc


@app.put("/skills/{skill_id}")
async def update_skill(skill_id: str, payload: UpdateSkillRequest) -> dict[str, Any]:
    try:
        return await service.update_skill(skill_id, payload.files)
    except HTTPException:
        raise
    except Exception as exc:
        status = _status_for_skill_error(exc)
        raise HTTPException(status_code=status, detail=str(exc)) from exc


@app.delete("/skills/{skill_id}", status_code=204)
async def delete_skill(skill_id: str) -> None:
    try:
        await service.delete_skill(skill_id)
    except HTTPException:
        raise
    except Exception as exc:
        status = _status_for_skill_error(exc)
        raise HTTPException(status_code=status, detail=str(exc)) from exc


def _status_for_skill_error(exc: Exception) -> int:
    """Map upstream httpx errors to appropriate status codes."""
    if isinstance(exc, httpx.HTTPStatusError):
        return exc.response.status_code
    return 500
