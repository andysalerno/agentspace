"""Thin client_service HTTP client used by gateway implementations.

Gateways are *clients* of client_service.  This wrapper hides the HTTP
plumbing so individual gateway implementations only deal with their
external system.
"""

from __future__ import annotations

import json as jsonlib
import logging
from dataclasses import dataclass
from typing import TYPE_CHECKING, cast

import httpx

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Mapping

logger = logging.getLogger(__name__)


class ClientServiceError(RuntimeError):
    pass


@dataclass(frozen=True, slots=True)
class ClientServiceClient:
    base_url: str
    timeout: float = 60.0

    async def create_session(
        self,
        *,
        agent_id: str,
        channel_name: str | None = None,
    ) -> dict[str, object]:
        payload: dict[str, object] = {"agent_id": agent_id}
        if channel_name is not None:
            payload["channel_name"] = channel_name
        return await self._request_json("POST", "/sessions", json=payload)

    async def send_message(
        self,
        *,
        session_id: str,
        message: str,
    ) -> dict[str, object]:
        return await self._request_json(
            "POST",
            f"/sessions/{session_id}/messages",
            json={"message": message},
        )

    async def reset_session(self, *, session_id: str) -> dict[str, object]:
        return await self._request_json("POST", f"/sessions/{session_id}/reset")

    def stream_message(
        self,
        *,
        session_id: str,
        message: str,
    ) -> AsyncIterator[dict[str, object]]:
        async def iterator() -> AsyncIterator[dict[str, object]]:
            path = f"/sessions/{session_id}/messages/stream"
            logger.info("gateway -> client_service POST %s", path)
            try:
                async with (
                    httpx.AsyncClient(
                        base_url=self.base_url,
                        timeout=self._httpx_timeout(),
                    ) as client,
                    client.stream(
                        "POST",
                        path,
                        json={"message": message},
                    ) as response,
                ):
                    response.raise_for_status()
                    async for line in response.aiter_lines():
                        if not line:
                            continue
                        data = jsonlib.loads(line)
                        if not isinstance(data, dict):
                            msg = (
                                f"unexpected stream item shape from {path}: "
                                f"{type(data).__name__}"
                            )
                            raise ClientServiceError(msg)
                        yield cast("dict[str, object]", data)
            except jsonlib.JSONDecodeError as exc:
                msg = f"invalid JSON stream item from {path}: {exc}"
                raise ClientServiceError(msg) from exc
            except httpx.HTTPError as exc:
                raise ClientServiceError(str(exc)) from exc

        return iterator()

    async def delete_session(self, *, session_id: str) -> None:
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self._httpx_timeout(),
        ) as client:
            response = await client.delete(f"/sessions/{session_id}")
        if response.status_code not in (200, 204, 404):
            response.raise_for_status()

    async def get_agent(self, agent_id: str) -> dict[str, object]:
        return await self._request_json("GET", f"/agents/{agent_id}")

    def _httpx_timeout(self) -> httpx.Timeout:
        # Reads have no deadline because client_service.send_message proxies a
        # streaming agent_host call that may take arbitrarily long while the
        # underlying agent works (web fetches, multi-step tool calls, etc.).
        # Connect/write timeouts still apply so we fail fast if the upstream
        # service is unreachable.
        return httpx.Timeout(self.timeout, read=None)

    async def _request_json(
        self,
        method: str,
        path: str,
        *,
        json: Mapping[str, object] | None = None,
    ) -> dict[str, object]:
        logger.info("gateway -> client_service %s %s", method, path)
        try:
            async with httpx.AsyncClient(
                base_url=self.base_url,
                timeout=self._httpx_timeout(),
            ) as client:
                response = await client.request(method, path, json=json)
            response.raise_for_status()
        except httpx.HTTPError as exc:
            raise ClientServiceError(str(exc)) from exc
        data = response.json()
        if not isinstance(data, dict):
            msg = f"unexpected response shape from {path}: {type(data).__name__}"
            raise ClientServiceError(msg)
        return cast("dict[str, object]", data)
