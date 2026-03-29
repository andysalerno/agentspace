from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import cast

import httpx

logger = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class SessionRegistration:
    session_id: str
    agent_id: str
    channel_name: str | None


@dataclass(frozen=True, slots=True)
class SessionReply:
    session_id: str
    assistant_text: str


@dataclass(frozen=True, slots=True)
class ClientServiceSessionClient:
    base_url: str
    timeout: float = 60.0

    async def create_session(
        self,
        *,
        agent_id: str,
        channel_name: str,
    ) -> SessionRegistration:
        response = await self._request_json(
            "POST",
            "/sessions",
            json={
                "agent_id": agent_id,
                "channel_name": channel_name,
                "client_type": "cli",
            },
        )
        return self._parse_session_registration(response)

    async def get_session(self, session_id: str) -> SessionRegistration:
        response = await self._request_json("GET", f"/sessions/{session_id}")
        return self._parse_session_registration(response)

    async def send_message(self, session_id: str, message: str) -> SessionReply:
        response = await self._request_json(
            "POST",
            f"/sessions/{session_id}/messages",
            json={"message": message},
        )
        data = cast("dict[str, object]", response)
        assistant_message = cast("dict[str, object]", data["assistant_message"])
        session = cast("dict[str, object]", data["session"])
        return SessionReply(
            session_id=str(session["session_id"]),
            assistant_text=str(assistant_message["content"]),
        )

    async def reset(self, session_id: str) -> SessionRegistration:
        response = await self._request_json("POST", f"/sessions/{session_id}/reset")
        return self._parse_session_registration(response)

    def _parse_session_registration(self, response: object) -> SessionRegistration:
        data = cast("dict[str, object]", response)
        channel_name = data.get("channel_name")
        return SessionRegistration(
            session_id=str(data["session_id"]),
            agent_id=str(data["agent_id"]),
            channel_name=None if channel_name is None else str(channel_name),
        )

    async def _request_json(
        self,
        method: str,
        path: str,
        *,
        json: dict[str, object] | None = None,
    ) -> object:
        logger.info("cli_channel -> client_service %s %s", method, path)
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout,
        ) as client:
            response = await client.request(method, path, json=json)
        response.raise_for_status()
        return response.json()
