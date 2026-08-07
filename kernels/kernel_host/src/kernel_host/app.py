from __future__ import annotations

import asyncio
import json
import logging
import os
import shutil
from contextlib import asynccontextmanager, suppress
from dataclasses import asdict
from typing import TYPE_CHECKING, Any

from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from fastapi.responses import StreamingResponse
from pydantic import BaseModel

from kernel_host.service import service_from_env
from kernel_host.terminal import (
    TerminalSession,
    terminal_authorized,
    valid_terminal_size,
)

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, AsyncIterator

    from kernel.events import KernelEvent

logger = logging.getLogger(__name__)


def _raise_terminal_task_error(error: BaseException) -> None:
    raise error


@asynccontextmanager
async def lifespan(_app: FastAPI) -> AsyncGenerator[None]:
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


class SendMessageRequest(BaseModel):
    message: str


def _serialize_events(events: list[KernelEvent]) -> list[dict[str, Any]]:
    return [asdict(event) for event in events]


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


@app.websocket("/terminal")
async def terminal(websocket: WebSocket, cols: int = 120, rows: int = 32) -> None:
    if not terminal_authorized(
        websocket.headers.get("authorization"),
        os.environ.get("KERNEL_HOST_TERMINAL_TOKEN"),
    ):
        await websocket.close(code=1008, reason="terminal authorization failed")
        return
    if not valid_terminal_size(cols, rows):
        await websocket.close(code=1008, reason="invalid terminal size")
        return

    await websocket.accept()
    session: TerminalSession | None = None
    try:
        session = await TerminalSession.open(
            cols,
            rows,
            workdir=os.environ.get("KERNEL_WORKDIR", "/workspace"),
        )
        receive_task = asyncio.create_task(_terminal_receive(websocket, session))
        send_task = asyncio.create_task(_terminal_send(websocket, session))
        _done, pending = await asyncio.wait(
            {receive_task, send_task},
            return_when=asyncio.FIRST_COMPLETED,
        )
        for task in pending:
            task.cancel()
        results = await asyncio.gather(
            receive_task,
            send_task,
            return_exceptions=True,
        )
        for result in results:
            if isinstance(result, (asyncio.CancelledError, WebSocketDisconnect)):
                continue
            if isinstance(result, BaseException):
                _raise_terminal_task_error(result)
    except WebSocketDisconnect:
        pass
    except Exception:
        logger.exception("interactive terminal failed")
        with suppress(RuntimeError, WebSocketDisconnect):
            await websocket.send_json(
                {"type": "error", "message": "interactive terminal failed"}
            )
    finally:
        if session is not None:
            await session.close()
        with suppress(RuntimeError, WebSocketDisconnect):
            await websocket.close()


async def _terminal_receive(
    websocket: WebSocket,
    session: TerminalSession,
) -> None:
    while True:
        message = await websocket.receive()
        if message["type"] == "websocket.disconnect":
            return
        if data := message.get("bytes"):
            await session.write(data)
            continue
        control = message.get("text")
        if not isinstance(control, str):
            continue
        try:
            payload = json.loads(control)
            if payload.get("type") != "resize":
                await websocket.send_json(
                    {
                        "type": "error",
                        "message": "unsupported terminal control message",
                    }
                )
                return
            cols = int(payload["cols"])
            rows = int(payload["rows"])
            await session.resize(cols, rows)
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            await websocket.send_json({"type": "error", "message": str(error)})
            return


async def _terminal_send(websocket: WebSocket, session: TerminalSession) -> None:
    while output := await session.read():
        await websocket.send_bytes(output)


@app.post("/reset")
async def reset() -> dict[str, Any]:
    return await service.reset()


@app.delete("/session", status_code=204)
async def stop() -> None:
    await service.stop()
