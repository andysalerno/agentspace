from __future__ import annotations

from dataclasses import dataclass, field
from datetime import UTC, datetime
from enum import StrEnum
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from gateway.protocol import GatewayType
    from kernel_host.registry import HarnessName


def utc_now() -> str:
    return datetime.now(UTC).isoformat()


class MessageRole(StrEnum):
    USER = "user"
    ASSISTANT = "assistant"
    SYSTEM = "system"


class ClientType(StrEnum):
    CLI = "cli"
    WEBUI = "webui"


def _empty_skills() -> list[str]:
    return []


@dataclass(frozen=True, slots=True)
class ToolCallRecord:
    tool: str
    input: str | None = None
    output: str | None = None

    def summary(self) -> dict[str, object]:
        data: dict[str, object] = {"tool": self.tool}
        if self.input is not None:
            data["input"] = self.input
        if self.output is not None:
            data["output"] = self.output
        return data


def _empty_tool_calls() -> list[ToolCallRecord]:
    return []


@dataclass(slots=True)
class AgentRecord:
    agent_id: str
    name: str
    harness: HarnessName
    system_prompt: str
    skills: list[str] = field(default_factory=_empty_skills)
    env_vars: str = ""
    created_at: str = field(default_factory=utc_now)
    updated_at: str = field(default_factory=utc_now)

    def summary(self) -> dict[str, object]:
        return {
            "agent_id": self.agent_id,
            "name": self.name,
            "harness": self.harness.value,
            "system_prompt": self.system_prompt,
            "skills": list(self.skills),
            "env_vars": self.env_vars,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        }


@dataclass(slots=True)
class KernelConfigRecord:
    harness: HarnessName
    env_vars: str = ""
    updated_at: str = field(default_factory=utc_now)

    def summary(self) -> dict[str, object]:
        return {
            "harness": self.harness.value,
            "env_vars": self.env_vars,
            "updated_at": self.updated_at,
        }


@dataclass(slots=True)
class MessageRecord:
    message_id: str
    session_id: str
    role: MessageRole
    content: str
    created_at: str = field(default_factory=utc_now)
    tool_calls: list[ToolCallRecord] = field(default_factory=_empty_tool_calls)
    reasoning: str = ""

    def summary(self) -> dict[str, object]:
        data: dict[str, object] = {
            "message_id": self.message_id,
            "session_id": self.session_id,
            "role": self.role.value,
            "content": self.content,
            "created_at": self.created_at,
        }
        if self.tool_calls:
            data["tool_calls"] = [tc.summary() for tc in self.tool_calls]
        if self.reasoning:
            data["reasoning"] = self.reasoning
        return data


def _empty_messages() -> list[MessageRecord]:
    return []


@dataclass(slots=True)
class SessionRecord:
    session_id: str
    agent_id: str
    agent_host_session_id: str
    status: str
    channel_name: str | None
    client_type: ClientType | None
    created_at: str = field(default_factory=utc_now)
    updated_at: str = field(default_factory=utc_now)
    messages: list[MessageRecord] = field(default_factory=_empty_messages)

    def summary(self) -> dict[str, object]:
        client_type = self.client_type.value if self.client_type is not None else None
        return {
            "session_id": self.session_id,
            "agent_id": self.agent_id,
            "agent_host_session_id": self.agent_host_session_id,
            "status": self.status,
            "channel_name": self.channel_name,
            "client_type": client_type,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "message_count": len(self.messages),
        }

    def detail(self) -> dict[str, object]:
        data = self.summary()
        data["messages"] = [message.summary() for message in self.messages]
        return data


def _empty_env_dict() -> dict[str, str]:
    return {}


@dataclass(slots=True)
class GatewayRecord:
    gateway_id: str
    name: str
    gateway_type: GatewayType
    agent_id: str
    enabled: bool
    env_vars: str = ""
    secrets: dict[str, str] = field(default_factory=_empty_env_dict)
    status: str = "stopped"
    last_error: str | None = None
    container_name: str | None = None
    created_at: str = field(default_factory=utc_now)
    updated_at: str = field(default_factory=utc_now)

    def summary(self, *, include_secrets: bool = False) -> dict[str, object]:
        data: dict[str, object] = {
            "gateway_id": self.gateway_id,
            "name": self.name,
            "gateway_type": self.gateway_type.value,
            "agent_id": self.agent_id,
            "enabled": self.enabled,
            "env_vars": self.env_vars,
            "status": self.status,
            "last_error": self.last_error,
            "container_name": self.container_name,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "secret_keys": sorted(self.secrets.keys()),
        }
        if include_secrets:
            data["secrets"] = dict(self.secrets)
        return data
