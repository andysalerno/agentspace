from __future__ import annotations

import asyncio
import logging
import os
import shutil
from contextlib import asynccontextmanager, suppress
from dataclasses import asdict
from typing import TYPE_CHECKING, Any

from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse
from pydantic import BaseModel, Field

from kernel_host.registry import HarnessName
from kernel_host.service import service_from_env
from kernel_host.terminal import (
    TerminalClientError,
    TerminalCommandError,
    TerminalConfigurationError,
    TerminalController,
    TerminalStateError,
    TerminalStatus,
    terminal_controller_from_env,
)

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, AsyncIterator, Awaitable, Callable

    from kernel.events import KernelEvent

logger = logging.getLogger(__name__)


@asynccontextmanager
async def lifespan(_app: FastAPI) -> AsyncGenerator[None]:
    if os.environ.get("KERNEL_HARNESS") == HarnessName.COPILOT_CLI:
        await _get_terminal_controller().validate_runtime()
    vscode_process = await _start_vscode_server()
    try:
        yield
    finally:
        if vscode_process is not None and vscode_process.returncode is None:
            vscode_process.terminate()
            with suppress(asyncio.TimeoutError):
                await asyncio.wait_for(vscode_process.wait(), timeout=5.0)
            if vscode_process.returncode is None:
                vscode_process.kill()
                await vscode_process.wait()


app = FastAPI(title="Kernel Host", version="0.1.0", lifespan=lifespan)
service = service_from_env()
terminal_controller: TerminalController | None = None


class SendMessageRequest(BaseModel):
    message: str


class CopyModeRequest(BaseModel):
    tmux_client_id: str = Field(
        min_length=1,
        max_length=256,
        description=(
            "Exact clients[].id value observed from GET /terminal. agent_host maps "
            "its attachment ID to this tmux client ID."
        ),
    )


def _serialize_events(events: list[KernelEvent]) -> list[dict[str, Any]]:
    return [asdict(event) for event in events]


def _get_terminal_controller() -> TerminalController:
    global terminal_controller  # noqa: PLW0603
    if terminal_controller is None:
        terminal_controller = terminal_controller_from_env()
    return terminal_controller


def _terminal_controller_for_request() -> TerminalController:
    try:
        return _get_terminal_controller()
    except TerminalConfigurationError as error:
        raise HTTPException(status_code=503, detail=str(error)) from error


def _serialize_terminal(status: TerminalStatus) -> dict[str, Any]:
    return {
        "state": status.state,
        "exit_status": status.exit_status,
        "attach_kind": status.attach_kind,
        "session_name": status.session_name,
        "target_session": status.target_session,
        "socket_path": status.socket_path,
        "attach_argv": list(status.attach_argv),
        "pane_id": status.pane_id,
        "pane_pid": status.pane_pid,
        "attachment_count": status.attachment_count,
        "clients": [asdict(client) for client in status.clients],
    }


async def _terminal_call(
    operation: str,
    call: Callable[[], Awaitable[TerminalStatus]],
) -> dict[str, Any]:
    try:
        status: TerminalStatus = await call()
    except TerminalClientError as error:
        raise HTTPException(status_code=404, detail=str(error)) from error
    except TerminalStateError as error:
        raise HTTPException(status_code=409, detail=str(error)) from error
    except (TerminalCommandError, TerminalConfigurationError) as error:
        logger.exception("terminal %s failed", operation)
        raise HTTPException(status_code=503, detail=str(error)) from error
    return _serialize_terminal(status)


async def _start_vscode_server() -> asyncio.subprocess.Process | None:
    enabled = os.environ.get("KERNEL_VSCODE_ENABLED", "1").lower()
    if enabled in {"0", "false", "no", "off"}:
        return None

    executable = shutil.which(os.environ.get("KERNEL_VSCODE_COMMAND", "code-server"))
    if executable is None:
        logger.warning("code-server executable not found; VS Code server disabled")
        return None

    bind_addr = os.environ.get("KERNEL_VSCODE_BIND_ADDR", "0.0.0.0:8080")
    workspace = os.environ.get("KERNEL_VSCODE_WORKDIR", "/workspace")
    auth = os.environ.get("KERNEL_VSCODE_AUTH", "none")
    args = [
        executable,
        "--bind-addr",
        bind_addr,
        "--auth",
        auth,
        "--disable-telemetry",
        workspace,
    ]
    logger.info("starting VS Code server: %s", " ".join(args))
    return await asyncio.create_subprocess_exec(*args)


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


@app.post("/messages/stream")
async def stream_message(payload: SendMessageRequest) -> StreamingResponse:
    async def event_lines() -> AsyncIterator[str]:
        async for event in service.stream_message(payload.message):
            yield f"{event.to_jsonl()}\n"

    return StreamingResponse(
        event_lines(),
        media_type="application/x-ndjson",
        headers={
            "Cache-Control": "no-cache",
            "X-Accel-Buffering": "no",
        },
    )


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


@app.post("/terminal/ensure")
async def terminal_ensure() -> dict[str, Any]:
    controller = _terminal_controller_for_request()
    return await _terminal_call("ensure", controller.ensure)


@app.get("/terminal")
async def terminal_status() -> dict[str, Any]:
    controller = _terminal_controller_for_request()
    return await _terminal_call("status", controller.status)


@app.post("/terminal/stop")
async def terminal_stop() -> dict[str, Any]:
    controller = _terminal_controller_for_request()
    return await _terminal_call("stop", controller.stop)


@app.post("/terminal/resume")
async def terminal_resume() -> dict[str, Any]:
    controller = _terminal_controller_for_request()
    return await _terminal_call("resume", controller.resume)


@app.post("/terminal/copy-mode")
async def terminal_copy_mode(payload: CopyModeRequest) -> dict[str, Any]:
    controller = _terminal_controller_for_request()

    async def enter_copy_mode() -> TerminalStatus:
        return await controller.copy_mode(payload.tmux_client_id)

    return await _terminal_call("copy-mode", enter_copy_mode)


@app.delete("/session", status_code=204)
async def stop() -> None:
    await service.stop()
