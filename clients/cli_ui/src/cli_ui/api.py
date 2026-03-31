"""HTTP client for the AgentSpace client-service API."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

import httpx

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator


@dataclass(frozen=True, slots=True)
class ApiClient:
    """Typed wrapper around the client-service REST API."""

    base_url: str = "http://127.0.0.1:8002"
    timeout: float = 120.0

    async def _request(
        self,
        method: str,
        path: str,
        *,
        json: dict[str, object] | None = None,
    ) -> Any:  # noqa: ANN401
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout,
        ) as client:
            resp = await client.request(method, f"/api{path}", json=json)
        resp.raise_for_status()
        if resp.status_code == 204:
            return None
        return resp.json()

    # ── Agents ──

    async def list_agents(self) -> list[dict[str, Any]]:
        return await self._request("GET", "/agents")  # type: ignore[return-value]

    async def create_agent(
        self,
        *,
        agent_id: str,
        name: str,
        system_prompt: str = "",
        skills: list[str] | None = None,
    ) -> dict[str, Any]:
        payload: dict[str, object] = {
            "agent_id": agent_id,
            "name": name,
            "system_prompt": system_prompt,
        }
        if skills:
            payload["skills"] = skills
        return await self._request("POST", "/agents", json=payload)  # type: ignore[return-value]

    async def delete_agent(self, agent_id: str) -> None:
        await self._request("DELETE", f"/agents/{agent_id}")

    # ── Sessions ──

    async def list_sessions(self) -> list[dict[str, Any]]:
        return await self._request("GET", "/sessions")  # type: ignore[return-value]

    async def get_session(self, session_id: str) -> dict[str, Any]:
        return await self._request("GET", f"/sessions/{session_id}")  # type: ignore[return-value]

    async def create_session(
        self,
        *,
        agent_id: str,
        channel_name: str | None = None,
    ) -> dict[str, Any]:
        return await self._request(  # type: ignore[return-value]
            "POST",
            "/sessions",
            json={
                "agent_id": agent_id,
                "channel_name": channel_name,
                "client_type": "cli",
            },
        )

    async def send_message(
        self,
        session_id: str,
        message: str,
    ) -> dict[str, Any]:
        return await self._request(  # type: ignore[return-value]
            "POST",
            f"/sessions/{session_id}/messages",
            json={"message": message},
        )

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[dict[str, Any]]:
        async def iterator() -> AsyncIterator[dict[str, Any]]:
            timeout = httpx.Timeout(self.timeout, read=None)
            async with (
                httpx.AsyncClient(
                base_url=self.base_url,
                timeout=timeout,
                ) as client,
                client.stream(
                    "POST",
                    f"/api/sessions/{session_id}/messages/stream",
                    json={"message": message},
                ) as resp,
            ):
                resp.raise_for_status()
                async for line in resp.aiter_lines():
                    if not line:
                        continue
                    payload = json.loads(line)
                    if isinstance(payload, dict):
                        yield payload

        return iterator()

    async def reset_session(self, session_id: str) -> dict[str, Any]:
        return await self._request("POST", f"/sessions/{session_id}/reset")  # type: ignore[return-value]

    async def delete_session(self, session_id: str) -> None:
        await self._request("DELETE", f"/sessions/{session_id}")

    # ── Kernels ──

    async def list_kernels(self) -> list[dict[str, Any]]:
        return await self._request("GET", "/kernels")  # type: ignore[return-value]

    async def kill_kernel(self, session_id: str) -> None:
        await self._request("DELETE", f"/kernels/{session_id}")

    async def kernel_logs(self, session_id: str) -> dict[str, Any]:
        return await self._request("GET", f"/kernels/{session_id}/logs")  # type: ignore[return-value]

    # ── Skills ──

    async def list_skills(self) -> list[dict[str, Any]]:
        return await self._request("GET", "/skills")  # type: ignore[return-value]

    async def get_skill(self, skill_id: str) -> dict[str, Any]:
        return await self._request("GET", f"/skills/{skill_id}")  # type: ignore[return-value]

    async def create_skill(
        self,
        *,
        skill_id: str,
        files: dict[str, str],
    ) -> dict[str, Any]:
        return await self._request(  # type: ignore[return-value]
            "POST",
            "/skills",
            json={"skill_id": skill_id, "files": files},
        )

    async def delete_skill(self, skill_id: str) -> None:
        await self._request("DELETE", f"/skills/{skill_id}")
