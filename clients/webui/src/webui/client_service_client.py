from __future__ import annotations

import logging
import os
from dataclasses import dataclass
from typing import TYPE_CHECKING, Protocol, cast

import httpx

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from collections.abc import Mapping

type JsonDict = dict[str, object]
type JsonList = list[JsonDict]
type JsonValue = JsonDict | JsonList


class ClientServiceClient(Protocol):
    async def list_agents(self) -> JsonList: ...

    async def create_agent(
        self,
        *,
        agent_id: str,
        name: str,
        system_prompt: str,
    ) -> JsonDict: ...

    async def list_sessions(self) -> JsonList: ...

    async def create_session(
        self,
        *,
        agent_id: str,
        cwd: str | None,
    ) -> JsonDict: ...

    async def get_session(self, session_id: str) -> JsonDict: ...

    async def send_message(self, session_id: str, message: str) -> JsonDict: ...

    async def reset_session(self, session_id: str) -> JsonDict: ...


@dataclass(frozen=True, slots=True)
class HttpClientServiceClient:
    base_url: str = os.environ.get(
        "WEBUI_CLIENT_SERVICE_BASE_URL",
        "http://127.0.0.1:8002",
    )
    timeout: float = float(os.environ.get("WEBUI_CLIENT_SERVICE_TIMEOUT", "60"))

    async def list_agents(self) -> JsonList:
        response = await self._request_json("GET", "/agents")
        return cast("JsonList", response)

    async def create_agent(
        self,
        *,
        agent_id: str,
        name: str,
        system_prompt: str,
    ) -> JsonDict:
        return cast(
            "JsonDict",
            await self._request_json(
                "POST",
                "/agents",
                json={
                    "agent_id": agent_id,
                    "name": name,
                    "system_prompt": system_prompt,
                },
            ),
        )

    async def list_sessions(self) -> JsonList:
        response = await self._request_json("GET", "/sessions")
        return cast("JsonList", response)

    async def create_session(self, *, agent_id: str, cwd: str | None) -> JsonDict:
        return cast(
            "JsonDict",
            await self._request_json(
                "POST",
                "/sessions",
                json={"agent_id": agent_id, "cwd": cwd},
            ),
        )

    async def get_session(self, session_id: str) -> JsonDict:
        response = await self._request_json("GET", f"/sessions/{session_id}")
        return cast("JsonDict", response)

    async def send_message(self, session_id: str, message: str) -> JsonDict:
        return cast(
            "JsonDict",
            await self._request_json(
                "POST",
                f"/sessions/{session_id}/messages",
                json={"message": message},
            ),
        )

    async def reset_session(self, session_id: str) -> JsonDict:
        return cast(
            "JsonDict",
            await self._request_json("POST", f"/sessions/{session_id}/reset"),
        )

    async def _request_json(
        self,
        method: str,
        path: str,
        *,
        json: Mapping[str, object] | None = None,
    ) -> JsonValue:
        logger.info("webui -> client_service %s %s", method, path)
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout,
        ) as client:
            response = await client.request(method, path, json=json)
        response.raise_for_status()
        return response.json()
