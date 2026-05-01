from __future__ import annotations

from dataclasses import dataclass, field
from datetime import UTC, datetime
from enum import StrEnum
from typing import TYPE_CHECKING, Literal

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
    tool_call_id: str | None = None
    status: str | None = None
    kind: str | None = None
    input: str | None = None
    output: str | None = None
    content_offset: int | None = None

    def summary(self) -> dict[str, object]:
        data: dict[str, object] = {"tool": self.tool}
        if self.tool_call_id is not None:
            data["tool_call_id"] = self.tool_call_id
        if self.status is not None:
            data["status"] = self.status
        if self.kind is not None:
            data["kind"] = self.kind
        if self.input is not None:
            data["input"] = self.input
        if self.output is not None:
            data["output"] = self.output
        if self.content_offset is not None:
            data["content_offset"] = self.content_offset
        return data


def _empty_tool_calls() -> list[ToolCallRecord]:
    return []


class WorkspaceMountMode(StrEnum):
    READ_WRITE = "rw"
    READ_ONLY = "ro"


@dataclass(frozen=True, slots=True)
class WorkspaceMountRecord:
    workspace_id: str
    mode: WorkspaceMountMode = WorkspaceMountMode.READ_WRITE

    def summary(self) -> dict[str, object]:
        return {
            "workspace_id": self.workspace_id,
            "mode": self.mode.value,
            "mount_path": f"/workspaces/{self.workspace_id}",
        }


def _empty_workspace_mounts() -> list[WorkspaceMountRecord]:
    return []


@dataclass(slots=True)
class AgentRecord:
    agent_id: str
    name: str
    harness: HarnessName
    system_prompt: str
    skills: list[str] = field(default_factory=_empty_skills)
    env_vars: str = ""
    connection_id: str | None = None
    workspace_mounts: list[WorkspaceMountRecord] = field(
        default_factory=_empty_workspace_mounts,
    )
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
            "connection_id": self.connection_id,
            "workspace_mounts": [mount.summary() for mount in self.workspace_mounts],
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
class WorkspaceRecord:
    workspace_id: str
    name: str
    created_at: str = field(default_factory=utc_now)
    updated_at: str = field(default_factory=utc_now)

    @property
    def volume_name(self) -> str:
        return f"agentspace-workspace-{self.workspace_id}"

    def summary(self) -> dict[str, object]:
        return {
            "workspace_id": self.workspace_id,
            "name": self.name,
            "mount_path": f"/workspaces/{self.workspace_id}",
            "volume_name": self.volume_name,
            "created_at": self.created_at,
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


ConnectionApiFlavor = Literal["chat_completions", "responses"]
DEFAULT_CONNECTION_API_FLAVOR: ConnectionApiFlavor = "chat_completions"


@dataclass(slots=True)
class ConnectionRecord:
    connection_id: str
    name: str
    url: str
    api_flavor: ConnectionApiFlavor = DEFAULT_CONNECTION_API_FLAVOR
    api_key: str = ""
    created_at: str = field(default_factory=utc_now)
    updated_at: str = field(default_factory=utc_now)

    def summary(self, *, include_api_key: bool = False) -> dict[str, object]:
        data: dict[str, object] = {
            "connection_id": self.connection_id,
            "name": self.name,
            "url": self.url,
            "api_flavor": self.api_flavor,
            "has_api_key": bool(self.api_key),
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        }
        if include_api_key:
            data["api_key"] = self.api_key
        return data


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
