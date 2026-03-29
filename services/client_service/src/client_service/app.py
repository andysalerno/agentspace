from __future__ import annotations

import logging
from typing import Any

from fastapi import FastAPI, HTTPException
from kernel_host.registry import HarnessName
from pydantic import BaseModel

from client_service.service import (
    AgentNotFoundError,
    ClientService,
    SessionNotFoundError,
)

logging.basicConfig(level=logging.INFO)

app = FastAPI(title="Client Service", version="0.1.0")
service = ClientService()


class CreateAgentRequest(BaseModel):
    name: str
    harness: HarnessName = HarnessName.COPILOT_CLI
    system_prompt: str = ""


class UpdateAgentRequest(BaseModel):
    name: str | None = None
    harness: HarnessName | None = None
    system_prompt: str | None = None


class CreateSessionRequest(BaseModel):
    agent_id: str
    cwd: str | None = None


class SendMessageRequest(BaseModel):
    message: str


@app.get("/healthz")
async def healthz() -> dict[str, str]:
    return {"status": "ok"}


@app.post("/agents")
async def create_agent(payload: CreateAgentRequest) -> dict[str, str]:
    return await service.create_agent(
        name=payload.name,
        harness=payload.harness,
        system_prompt=payload.system_prompt,
    )


@app.get("/agents")
async def list_agents() -> list[dict[str, str]]:
    return await service.list_agents()


@app.get("/agents/{agent_id}")
async def get_agent(agent_id: str) -> dict[str, str]:
    try:
        return await service.get_agent(agent_id)
    except AgentNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.patch("/agents/{agent_id}")
async def update_agent(
    agent_id: str,
    payload: UpdateAgentRequest,
) -> dict[str, str]:
    try:
        return await service.update_agent(
            agent_id,
            name=payload.name,
            harness=payload.harness,
            system_prompt=payload.system_prompt,
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
        return await service.create_session(agent_id=payload.agent_id, cwd=payload.cwd)
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
async def list_messages(session_id: str) -> dict[str, list[dict[str, str]]]:
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
