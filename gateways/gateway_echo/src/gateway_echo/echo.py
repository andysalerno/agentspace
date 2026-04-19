"""Echo gateway implementation."""

from __future__ import annotations

import asyncio
import logging
from collections import deque
from dataclasses import dataclass
from typing import TYPE_CHECKING, cast

from fastapi import APIRouter, HTTPException
from gateway.events import GatewayEvent, GatewayEventType, utc_now_iso
from gateway.protocol import GatewayConfig, GatewayStatus, GatewayType
from pydantic import BaseModel

if TYPE_CHECKING:
    from gateway.client import ClientServiceClient

logger = logging.getLogger(__name__)

OUTBOX_LIMIT = 100


class InboxRequest(BaseModel):
    sender: str
    text: str


@dataclass(slots=True)
class OutboxEntry:
    sender: str
    text: str
    reply: str
    session_id: str
    ts: str


class EchoGateway:
    """Reference gateway that exposes a synthetic inbox/outbox over HTTP."""

    def __init__(self) -> None:
        self._status = GatewayStatus.STOPPED
        self._last_error: str | None = None
        self._config: GatewayConfig | None = None
        # sender -> client_service session_id (one session per sender)
        self._sessions: dict[str, str] = {}
        self._outbox: deque[OutboxEntry] = deque(maxlen=OUTBOX_LIMIT)
        self._events: deque[GatewayEvent] = deque(maxlen=OUTBOX_LIMIT)
        self._send_lock = asyncio.Lock()

    @property
    def name(self) -> str:
        return GatewayType.ECHO.value

    @property
    def status(self) -> GatewayStatus:
        return self._status

    @property
    def last_error(self) -> str | None:
        return self._last_error

    async def start(self, config: GatewayConfig) -> None:
        self._config = config
        self._status = GatewayStatus.RUNNING
        self._last_error = None
        self._record_event(
            GatewayEvent(
                type=GatewayEventType.STATUS,
                message="echo gateway started",
            ),
        )
        logger.info(
            "echo gateway started: gateway_id=%s agent_id=%s",
            config.gateway_id,
            config.agent_id,
        )

    async def stop(self) -> None:
        self._status = GatewayStatus.STOPPED
        self._record_event(
            GatewayEvent(
                type=GatewayEventType.STATUS,
                message="echo gateway stopped",
            ),
        )
        logger.info("echo gateway stopped")

    def extra_router(self) -> APIRouter:
        router = APIRouter(prefix="/gateway", tags=["echo-gateway"])

        @router.post("/inbox")
        async def inbox(payload: InboxRequest) -> dict[str, object]:
            try:
                entry = await self._handle_inbox(payload.sender, payload.text)
            except RuntimeError as exc:
                raise HTTPException(status_code=503, detail=str(exc)) from exc
            return {
                "sender": entry.sender,
                "text": entry.text,
                "reply": entry.reply,
                "session_id": entry.session_id,
                "ts": entry.ts,
            }

        @router.get("/outbox")
        async def outbox() -> dict[str, object]:
            return {
                "entries": [
                    {
                        "sender": entry.sender,
                        "text": entry.text,
                        "reply": entry.reply,
                        "session_id": entry.session_id,
                        "ts": entry.ts,
                    }
                    for entry in self._outbox
                ],
            }

        @router.get("/events")
        async def events() -> dict[str, object]:
            return {"events": [event.to_dict() for event in self._events]}

        # Reference handler functions so static analyzers don't flag them as
        # unused (they are registered with FastAPI via the decorators above).
        _ = (inbox, outbox, events)
        return router

    async def _handle_inbox(self, sender: str, text: str) -> OutboxEntry:
        if self._config is None or self._status is not GatewayStatus.RUNNING:
            msg = "echo gateway is not running"
            raise RuntimeError(msg)
        config = self._config

        self._record_event(
            GatewayEvent(
                type=GatewayEventType.INBOUND,
                sender=sender,
                content=text,
            ),
        )

        async with self._send_lock:
            session_id = await self._ensure_session(
                config.client,
                config.agent_id,
                sender,
            )
            try:
                response = await config.client.send_message(
                    session_id=session_id,
                    message=text,
                )
            except Exception as exc:
                self._last_error = str(exc)
                self._status = GatewayStatus.ERROR
                self._record_event(
                    GatewayEvent(
                        type=GatewayEventType.ERROR,
                        sender=sender,
                        message=str(exc),
                        session_id=session_id,
                    ),
                )
                logger.exception(
                    "echo gateway send_message failed; transitioning to ERROR "
                    "(restart required to recover)",
                )
                raise

        assistant = response.get("assistant_message")
        reply = ""
        if isinstance(assistant, dict):
            content = cast("dict[str, object]", assistant).get("content")
            if isinstance(content, str):
                reply = content

        entry = OutboxEntry(
            sender=sender,
            text=text,
            reply=reply,
            session_id=session_id,
            ts=utc_now_iso(),
        )
        self._outbox.append(entry)
        self._record_event(
            GatewayEvent(
                type=GatewayEventType.OUTBOUND,
                sender=sender,
                content=reply,
                session_id=session_id,
            ),
        )
        return entry

    async def _ensure_session(
        self,
        client: ClientServiceClient,
        agent_id: str,
        sender: str,
    ) -> str:
        existing = self._sessions.get(sender)
        if existing is not None:
            return existing
        session = await client.create_session(
            agent_id=agent_id,
            channel_name=f"echo:{sender}",
        )
        session_id = str(session["session_id"])
        self._sessions[sender] = session_id
        return session_id

    def _record_event(self, event: GatewayEvent) -> None:
        self._events.append(event)
