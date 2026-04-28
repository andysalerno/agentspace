from __future__ import annotations

import json
import logging
import os
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Protocol, cast

import httpx
from kernel.events import KernelEvent, KernelStatus

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Mapping

    from kernel_host.registry import HarnessName

logger = logging.getLogger(__name__)

type JsonDict = dict[str, object]
type JsonList = list[JsonDict]
type JsonValue = JsonDict | JsonList


class AgentHostClient(Protocol):
    async def create_session(
        self,
        *,
        harness: HarnessName,
        skills: list[str] | None = None,
        env: dict[str, str] | None = None,
    ) -> JsonDict: ...

    async def get_session(self, session_id: str) -> JsonDict: ...

    async def list_sessions(self, *, with_stats: bool = False) -> JsonList: ...

    async def send_message(
        self,
        session_id: str,
        message: str,
    ) -> list[KernelEvent]: ...

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[KernelEvent]: ...

    async def history(self, session_id: str) -> list[list[KernelEvent]]: ...

    async def logs(self, session_id: str) -> list[str]: ...

    async def container_logs(
        self,
        session_id: str,
        *,
        tail: int | None,
    ) -> list[str]: ...

    async def reset_session(self, session_id: str) -> JsonDict: ...

    async def destroy_session(self, session_id: str) -> None: ...

    async def create_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> JsonDict: ...

    async def get_skill(self, skill_id: str) -> JsonDict: ...

    async def list_skills(self) -> JsonList: ...

    async def update_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> JsonDict: ...

    async def delete_skill(self, skill_id: str) -> None: ...

    async def info(self) -> JsonDict: ...

    async def create_gateway(
        self,
        *,
        gateway_id: str,
        gateway_type: str,
        agent_id: str,
        env: dict[str, str],
    ) -> JsonDict: ...

    async def list_gateways(self) -> JsonList: ...

    async def get_gateway(self, gateway_id: str) -> JsonDict: ...

    async def gateway_logs(self, gateway_id: str) -> list[str]: ...

    async def destroy_gateway(self, gateway_id: str) -> None: ...


@dataclass(frozen=True, slots=True)
class HttpAgentHostClient:
    base_url: str = os.environ.get(
        "CLIENT_SERVICE_AGENT_HOST_BASE_URL",
        "http://127.0.0.1:8001",
    )
    timeout: float = float(os.environ.get("CLIENT_SERVICE_AGENT_HOST_TIMEOUT", "60"))

    async def create_session(
        self,
        *,
        harness: HarnessName,
        skills: list[str] | None = None,
        env: dict[str, str] | None = None,
    ) -> JsonDict:
        payload: dict[str, object] = {"harness": harness.value}
        if skills is not None:
            payload["skills"] = skills
        if env:
            payload["env"] = env
        return cast(
            "JsonDict",
            await self._request_json("POST", "/sessions", json=payload),
        )

    async def get_session(self, session_id: str) -> JsonDict:
        response = await self._request_json("GET", f"/sessions/{session_id}")
        return cast("JsonDict", response)

    async def list_sessions(self, *, with_stats: bool = False) -> JsonList:
        path = "/sessions?with_stats=true" if with_stats else "/sessions"
        return cast("JsonList", await self._request_json("GET", path))

    async def send_message(self, session_id: str, message: str) -> list[KernelEvent]:
        return [event async for event in self.stream_message(session_id, message)]

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[KernelEvent]:
        async def iterator() -> AsyncIterator[KernelEvent]:
            timeout = httpx.Timeout(self.timeout, read=None)
            async with (
                httpx.AsyncClient(
                    base_url=self.base_url,
                    timeout=timeout,
                ) as client,
                client.stream(
                    "POST",
                    f"/sessions/{session_id}/messages/stream",
                    json={"message": message},
                ) as response,
            ):
                response.raise_for_status()
                async for line in response.aiter_lines():
                    if not line:
                        continue
                    raw_event = json.loads(line)
                    if not isinstance(raw_event, dict):
                        continue
                    yield _kernel_event_from_json(cast("JsonDict", raw_event))

        return iterator()

    async def history(self, session_id: str) -> list[list[KernelEvent]]:
        response = await self._request_json("GET", f"/sessions/{session_id}/history")
        raw_history = cast(
            "list[list[JsonDict]]",
            cast("JsonDict", response)["history"],
        )
        return [
            [_kernel_event_from_json(event) for event in turn] for turn in raw_history
        ]

    async def logs(self, session_id: str) -> list[str]:
        response = await self._request_json(
            "GET",
            f"/sessions/{session_id}/logs",
        )
        return cast("list[str]", cast("JsonDict", response)["lines"])

    async def container_logs(
        self,
        session_id: str,
        *,
        tail: int | None,
    ) -> list[str]:
        path = f"/sessions/{session_id}/container-logs"
        if tail is not None:
            path = f"{path}?tail={tail}"
        response = await self._request_json("GET", path)
        return cast("list[str]", cast("JsonDict", response)["lines"])

    async def reset_session(self, session_id: str) -> JsonDict:
        return cast(
            "JsonDict",
            await self._request_json("POST", f"/sessions/{session_id}/reset"),
        )

    async def destroy_session(self, session_id: str) -> None:
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout,
        ) as client:
            response = await client.delete(f"/sessions/{session_id}")
        response.raise_for_status()

    async def create_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> JsonDict:
        payload: dict[str, object] = {"skill_id": skill_id, "files": files}
        return cast(
            "JsonDict",
            await self._request_json("POST", "/skills", json=payload),
        )

    async def get_skill(self, skill_id: str) -> JsonDict:
        return cast("JsonDict", await self._request_json("GET", f"/skills/{skill_id}"))

    async def list_skills(self) -> JsonList:
        return cast("JsonList", await self._request_json("GET", "/skills"))

    async def update_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> JsonDict:
        payload: dict[str, object] = {"files": files}
        return cast(
            "JsonDict",
            await self._request_json("PUT", f"/skills/{skill_id}", json=payload),
        )

    async def delete_skill(self, skill_id: str) -> None:
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout,
        ) as client:
            response = await client.delete(f"/skills/{skill_id}")
        response.raise_for_status()

    async def info(self) -> JsonDict:
        return cast("JsonDict", await self._request_json("GET", "/info"))

    async def create_gateway(
        self,
        *,
        gateway_id: str,
        gateway_type: str,
        agent_id: str,
        env: dict[str, str],
    ) -> JsonDict:
        payload: dict[str, object] = {
            "gateway_id": gateway_id,
            "gateway_type": gateway_type,
            "agent_id": agent_id,
            "env": env,
        }
        return cast(
            "JsonDict",
            await self._request_json("POST", "/gateways", json=payload),
        )

    async def list_gateways(self) -> JsonList:
        return cast("JsonList", await self._request_json("GET", "/gateways"))

    async def get_gateway(self, gateway_id: str) -> JsonDict:
        return cast(
            "JsonDict",
            await self._request_json("GET", f"/gateways/{gateway_id}"),
        )

    async def gateway_logs(self, gateway_id: str) -> list[str]:
        response = await self._request_json(
            "GET",
            f"/gateways/{gateway_id}/logs",
        )
        return cast("list[str]", cast("JsonDict", response)["lines"])

    async def destroy_gateway(self, gateway_id: str) -> None:
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout,
        ) as client:
            response = await client.delete(f"/gateways/{gateway_id}")
        if response.status_code not in (200, 204, 404):
            response.raise_for_status()

    async def _request_json(
        self,
        method: str,
        path: str,
        *,
        json: Mapping[str, object] | None = None,
    ) -> JsonValue:
        logger.info("client_service -> agent_host %s %s", method, path)
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout,
        ) as client:
            response = await client.request(method, path, json=json)
        response.raise_for_status()
        return response.json()


def _kernel_event_from_json(event: JsonDict) -> KernelEvent:
    event_type = event.get("type")
    if not isinstance(event_type, str):
        event_type = str(event_type)
    ts = event.get("ts")
    raw_status = event.get("status")
    status = KernelStatus(raw_status) if isinstance(raw_status, str) else None
    return KernelEvent(
        type=event_type,
        ts=ts if isinstance(ts, str) else KernelEvent(type=event_type).ts,
        session_id=_optional_str(event.get("session_id")),
        kernel=_optional_str(event.get("kernel")),
        status=status,
        method=_optional_str(event.get("method")),
        params=_optional_dict(event.get("params")),
        update=_optional_dict(event.get("update")),
        result=_optional_dict(event.get("result")),
        error=_optional_dict(event.get("error")),
        content=_optional_str(event.get("content")),
        tool=_optional_str(event.get("tool")),
        input=_optional_dict(event.get("input")),
        output=_optional_str(event.get("output")),
        message=_optional_str(event.get("message")),
    )


def _optional_str(value: object) -> str | None:
    return value if isinstance(value, str) else None


def _optional_dict(value: object) -> dict[str, Any] | None:
    if isinstance(value, dict):
        return cast("dict[str, Any]", value)
    return None
