from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Any

import httpx
from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse
from gateway.protocol import GatewayType
from kernel_host.registry import HarnessName
from pydantic import BaseModel, Field

from client_service.models import ClientType  # noqa: TC001
from client_service.service import (
    AgentAlreadyExistsError,
    AgentNotFoundError,
    ClientService,
    GatewayAlreadyExistsError,
    GatewayNotFoundError,
    InvalidAgentIdError,
    InvalidGatewayIdError,
    KernelNotFoundError,
    SessionNotFoundError,
)
from client_service.storage import (
    Database,
    SqliteAgentStore,
    SqliteGatewayStore,
    SqliteKernelConfigStore,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

logger = logging.getLogger(__name__)


def _build_service() -> tuple[ClientService, Database | None]:
    db_path = os.environ.get("CLIENT_SERVICE_DB_PATH")
    if not db_path:
        logger.info(
            "CLIENT_SERVICE_DB_PATH unset; using in-memory agent store",
        )
        return ClientService(), None
    database = Database(db_path)
    agent_store = SqliteAgentStore(database)
    kernel_config_store = SqliteKernelConfigStore(database)
    gateway_store = SqliteGatewayStore(database)
    return (
        ClientService(
            agent_store=agent_store,
            kernel_config_store=kernel_config_store,
            gateway_store=gateway_store,
        ),
        database,
    )


service, _database = _build_service()


@asynccontextmanager
async def lifespan(_app: FastAPI) -> AsyncIterator[None]:
    if _database is not None:
        await _database.connect()
        await SqliteAgentStore(_database).initialize()
        await SqliteKernelConfigStore(_database).initialize()
        await SqliteGatewayStore(_database).initialize()
    # Run autostart concurrently with serving so a slow agent_host or
    # transient container failure does not block the API from coming up.
    autostart_task = asyncio.create_task(
        service.autostart_enabled_gateways(),
        name="gateway-autostart",
    )
    try:
        yield
    finally:
        if not autostart_task.done():
            autostart_task.cancel()
        with contextlib.suppress(asyncio.CancelledError, Exception):
            await autostart_task
        if _database is not None:
            await _database.close()


app = FastAPI(title="Client Service", version="0.1.0", lifespan=lifespan)
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=False,
    allow_methods=["*"],
    allow_headers=["*"],
)


class CreateAgentRequest(BaseModel):
    agent_id: str = Field(pattern=r"^[a-z]+(?:-[a-z]+)*$")
    name: str
    harness: HarnessName = HarnessName.COPILOT_CLI
    system_prompt: str = ""
    skills: list[str] = Field(default_factory=list)
    env_vars: str = ""


class UpdateAgentRequest(BaseModel):
    name: str | None = None
    harness: HarnessName | None = None
    system_prompt: str | None = None
    skills: list[str] | None = None
    env_vars: str | None = None


class CreateSessionRequest(BaseModel):
    agent_id: str
    channel_name: str | None = None
    client_type: ClientType | None = None


class SendMessageRequest(BaseModel):
    message: str


@app.get("/healthz")
async def healthz() -> dict[str, str]:
    return {"status": "ok"}


@app.get("/info")
async def info() -> dict[str, object]:
    return await service.info()


@app.get("/harnesses")
async def list_harnesses() -> list[str]:
    return await service.list_harnesses()


class UpdateKernelConfigRequest(BaseModel):
    env_vars: str = ""


@app.get("/kernel-configs")
async def list_kernel_configs() -> list[dict[str, object]]:
    return await service.list_kernel_configs()


@app.get("/kernel-configs/{harness}")
async def get_kernel_config(harness: HarnessName) -> dict[str, object]:
    return await service.get_kernel_config(harness)


@app.put("/kernel-configs/{harness}")
async def update_kernel_config(
    harness: HarnessName,
    payload: UpdateKernelConfigRequest,
) -> dict[str, object]:
    return await service.update_kernel_config(harness, payload.env_vars)


@app.post("/agents")
async def create_agent(payload: CreateAgentRequest) -> dict[str, object]:
    try:
        return await service.create_agent(
            agent_id=payload.agent_id,
            name=payload.name,
            harness=payload.harness,
            system_prompt=payload.system_prompt,
            skills=payload.skills,
            env_vars=payload.env_vars,
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
            env_vars=payload.env_vars,
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


# --- Gateways ---


class CreateGatewayRequest(BaseModel):
    gateway_id: str = Field(pattern=r"^[a-z]+(?:-[a-z]+)*$")
    name: str
    gateway_type: GatewayType
    agent_id: str
    enabled: bool = False
    env_vars: str = ""
    secrets: dict[str, str] = Field(default_factory=dict[str, str])


class UpdateGatewayRequest(BaseModel):
    name: str | None = None
    agent_id: str | None = None
    enabled: bool | None = None
    env_vars: str | None = None
    secrets: dict[str, str] | None = None


@app.get("/gateway-types")
async def list_gateway_types() -> list[str]:
    return [gateway_type.value for gateway_type in GatewayType]


@app.get("/gateways")
async def list_gateways() -> list[dict[str, Any]]:
    return await service.list_gateways()


@app.post("/gateways")
async def create_gateway(payload: CreateGatewayRequest) -> dict[str, Any]:
    try:
        return await service.create_gateway(
            gateway_id=payload.gateway_id,
            name=payload.name,
            gateway_type=payload.gateway_type,
            agent_id=payload.agent_id,
            enabled=payload.enabled,
            env_vars=payload.env_vars,
            secrets=payload.secrets,
        )
    except GatewayAlreadyExistsError as exc:
        raise HTTPException(status_code=409, detail=str(exc)) from exc
    except InvalidGatewayIdError as exc:
        raise HTTPException(status_code=422, detail=str(exc)) from exc
    except AgentNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.get("/gateways/{gateway_id}")
async def get_gateway(gateway_id: str) -> dict[str, Any]:
    try:
        return await service.get_gateway(gateway_id)
    except GatewayNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.patch("/gateways/{gateway_id}")
async def update_gateway(
    gateway_id: str,
    payload: UpdateGatewayRequest,
) -> dict[str, Any]:
    try:
        return await service.update_gateway(
            gateway_id,
            name=payload.name,
            agent_id=payload.agent_id,
            enabled=payload.enabled,
            env_vars=payload.env_vars,
            secrets=payload.secrets,
        )
    except GatewayNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    except AgentNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.delete("/gateways/{gateway_id}", status_code=204)
async def delete_gateway(gateway_id: str) -> None:
    try:
        await service.delete_gateway(gateway_id)
    except GatewayNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.post("/gateways/{gateway_id}/start")
async def start_gateway(gateway_id: str) -> dict[str, Any]:
    try:
        return await service.start_gateway(gateway_id)
    except GatewayNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.post("/gateways/{gateway_id}/stop")
async def stop_gateway(gateway_id: str) -> dict[str, Any]:
    try:
        return await service.stop_gateway(gateway_id)
    except GatewayNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc


@app.get("/gateways/{gateway_id}/logs")
async def gateway_logs(gateway_id: str) -> dict[str, list[str]]:
    try:
        lines = await service.gateway_logs(gateway_id)
    except GatewayNotFoundError as exc:
        raise HTTPException(status_code=404, detail=str(exc)) from exc
    return {"lines": lines}

