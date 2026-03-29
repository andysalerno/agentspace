from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import cast

import httpx

logger = logging.getLogger(__name__)


@dataclass(frozen=True, slots=True)
class ChannelRegistration:
    channel_id: str
    session_id: str
    name: str


@dataclass(frozen=True, slots=True)
class ChannelReply:
    session_id: str
    assistant_text: str


@dataclass(frozen=True, slots=True)
class ClientServiceChannelClient:
    base_url: str
    timeout: float = 60.0

    async def register_channel(
        self,
        *,
        agent_id: str,
        name: str,
        cwd: str | None,
    ) -> ChannelRegistration:
        response = await self._request_json(
            "POST",
            "/channels",
            json={
                "agent_id": agent_id,
                "name": name,
                "channel_type": "cli",
                "cwd": cwd,
            },
        )
        data = cast("dict[str, object]", response)
        return ChannelRegistration(
            channel_id=str(data["channel_id"]),
            session_id=str(data["session_id"]),
            name=str(data["name"]),
        )

    async def send_message(self, channel_id: str, message: str) -> ChannelReply:
        response = await self._request_json(
            "POST",
            f"/channels/{channel_id}/messages",
            json={"message": message},
        )
        data = cast("dict[str, object]", response)
        assistant_message = cast("dict[str, object]", data["assistant_message"])
        session = cast("dict[str, object]", data["session"])
        return ChannelReply(
            session_id=str(session["session_id"]),
            assistant_text=str(assistant_message["content"]),
        )

    async def reset(self, channel_id: str) -> ChannelRegistration:
        response = await self._request_json("POST", f"/channels/{channel_id}/reset")
        data = cast("dict[str, object]", response)
        return ChannelRegistration(
            channel_id=str(data["channel_id"]),
            session_id=str(data["session_id"]),
            name=str(data["name"]),
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
