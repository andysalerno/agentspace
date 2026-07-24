from __future__ import annotations

import logging
from dataclasses import dataclass
from email.message import Message
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
class ConfigDownload:
    content: bytes
    filename: str
    content_type: str


@dataclass(frozen=True, slots=True)
class SecretStatus:
    name: str
    description: str | None
    is_set: bool
    references: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class ClientServiceSessionClient:
    base_url: str
    timeout: float = 60.0
    transport: httpx.AsyncBaseTransport | None = None

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

    async def validate_config(self, source: bytes) -> object:
        return await self._request_json(
            "POST",
            "/config/validate",
            content=source,
        )

    async def plan_config(self, source: bytes) -> object:
        return await self._request_json(
            "POST",
            "/config/plan",
            content=source,
        )

    async def apply_config(self, source: bytes) -> object:
        return await self._request_json(
            "POST",
            "/config/apply",
            content=source,
        )

    async def export_config(self, mode: str) -> ConfigDownload:
        return await self._download("/config/export", params={"mode": mode})

    async def export_resource(self, kind: str, name: str) -> ConfigDownload:
        return await self._download(f"/config/export/{kind}/{name}")

    async def list_secrets(self) -> list[SecretStatus]:
        response = await self._request_json("GET", "/secrets")
        values = cast("list[dict[str, object]]", response)
        return [_parse_secret_status(value) for value in values]

    async def set_secret_value(self, name: str, value: str) -> None:
        await self._request(
            "PUT",
            f"/secrets/{name}/value",
            json={"value": value},
        )

    async def clear_secret_value(self, name: str) -> None:
        await self._request("DELETE", f"/secrets/{name}/value")

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
        content: bytes | None = None,
    ) -> object:
        response = await self._request(
            method,
            path,
            json=json,
            content=content,
        )
        return response.json()

    async def _download(
        self,
        path: str,
        *,
        params: dict[str, str] | None = None,
    ) -> ConfigDownload:
        response = await self._request("GET", path, params=params)
        return ConfigDownload(
            content=response.content,
            filename=_response_filename(response),
            content_type=response.headers.get(
                "content-type",
                "application/octet-stream",
            ),
        )

    async def _request(
        self,
        method: str,
        path: str,
        *,
        json: dict[str, object] | None = None,
        content: bytes | None = None,
        params: dict[str, str] | None = None,
    ) -> httpx.Response:
        logger.info("cli_channel -> client_service %s %s", method, path)
        headers = None if content is None else {"content-type": "application/yaml"}
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout,
            transport=self.transport,
        ) as client:
            response = await client.request(
                method,
                path,
                json=json,
                content=content,
                headers=headers,
                params=params,
            )
        response.raise_for_status()
        return response


def _response_filename(response: httpx.Response) -> str:
    disposition = response.headers.get("content-disposition")
    if disposition is None:
        return "agentspace-config.yaml"
    message = Message()
    message["content-disposition"] = disposition
    filename = message.get_filename()
    return filename or "agentspace-config.yaml"


def _parse_secret_status(value: dict[str, object]) -> SecretStatus:
    raw_references = value.get("references")
    references = (
        cast("list[object]", raw_references) if isinstance(raw_references, list) else []
    )
    raw_is_set = value["is_set"]
    if not isinstance(raw_is_set, bool):
        msg = "secret is_set must be a boolean"
        raise TypeError(msg)
    return SecretStatus(
        name=str(value["name"]),
        description=(
            None if value.get("description") is None else str(value["description"])
        ),
        is_set=raw_is_set,
        references=tuple(str(item) for item in references),
    )
