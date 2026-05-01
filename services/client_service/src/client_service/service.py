from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import re
import uuid
from dataclasses import asdict, dataclass, field, replace
from time import perf_counter
from typing import TYPE_CHECKING, cast

import httpx
from kernel.events import EventType, KernelEvent
from kernel_host.registry import HarnessName, available_harnesses

from client_service.agent_host_client import AgentHostClient, HttpAgentHostClient
from client_service.models import (
    DEFAULT_CONNECTION_API_FLAVOR,
    AgentRecord,
    ClientType,
    ConnectionApiFlavor,
    ConnectionRecord,
    GatewayRecord,
    MessageRecord,
    MessageRole,
    SessionRecord,
    ToolCallRecord,
    WorkspaceMountMode,
    WorkspaceMountRecord,
    WorkspaceRecord,
    utc_now,
)
from client_service.storage.agents import (
    AgentExistsError,
    AgentMissingError,
    AgentStore,
    InMemoryAgentStore,
)
from client_service.storage.connections import (
    ConnectionExistsError,
    ConnectionMissingError,
    ConnectionStore,
    InMemoryConnectionStore,
)
from client_service.storage.gateways import (
    GatewayExistsError,
    GatewayStore,
    InMemoryGatewayStore,
)
from client_service.storage.kernel_configs import (
    InMemoryKernelConfigStore,
    KernelConfigStore,
)
from client_service.storage.sessions import InMemorySessionStore, SessionStore
from client_service.storage.workspaces import (
    InMemoryWorkspaceStore,
    WorkspaceExistsError,
    WorkspaceMissingError,
    WorkspaceStore,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Awaitable, Callable

    from gateway.protocol import GatewayType

    type AcloseFn = Callable[[], Awaitable[object]]
    type StreamChunk = dict[str, object]


def _empty_events() -> list[KernelEvent]:
    return []


def _empty_subscribers() -> set[asyncio.Queue[StreamChunk | None]]:
    return set()


@dataclass(slots=True)
class _ActiveTurn:
    turn_id: str
    session_id: str
    agent_host_session_id: str
    message: str
    user_message: MessageRecord
    assistant_message: MessageRecord
    events: list[KernelEvent] = field(default_factory=_empty_events)
    subscribers: set[asyncio.Queue[StreamChunk | None]] = field(
        default_factory=_empty_subscribers,
    )
    task: asyncio.Task[None] | None = None
    final_payload: StreamChunk | None = None
    error: str | None = None


logger = logging.getLogger(__name__)
AGENT_ID_PATTERN = re.compile(r"^[a-z]+(?:-[a-z]+)*$")
CONNECTION_ID_PATTERN = re.compile(r"^[a-z]+(?:-[a-z]+)*$")
GATEWAY_ID_PATTERN = re.compile(r"^[a-z]+(?:-[a-z]+)*$")
WORKSPACE_ID_PATTERN = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
_UNSPECIFIED: object = object()
CLIENT_SERVICE_ENV_PREFIX = "CLIENT_SERVICE_"


class AgentNotFoundError(KeyError):
    pass


class AgentAlreadyExistsError(ValueError):
    pass


class InvalidAgentIdError(ValueError):
    pass


class SessionNotFoundError(KeyError):
    pass


class KernelNotFoundError(KeyError):
    pass


class GatewayNotFoundError(KeyError):
    pass


class GatewayAlreadyExistsError(ValueError):
    pass


class InvalidGatewayIdError(ValueError):
    pass


class ConnectionNotFoundError(KeyError):
    pass


class ConnectionAlreadyExistsError(ValueError):
    pass


class InvalidConnectionIdError(ValueError):
    pass


class WorkspaceNotFoundError(KeyError):
    pass


class WorkspaceAlreadyExistsError(ValueError):
    pass


class WorkspaceInUseError(ValueError):
    pass


class InvalidWorkspaceIdError(ValueError):
    pass


class ConnectionModelsError(RuntimeError):
    pass


VISIBLE_ASSISTANT_EVENT_TYPES = frozenset(
    {
        EventType.SESSION_UPDATE,
        EventType.SESSION_ERROR,
        EventType.TEXT_DELTA,
        EventType.REASONING_DELTA,
        EventType.TOOL_CALL,
        EventType.TOOL_RESULT,
        EventType.ERROR,
    },
)


class ClientService:
    def __init__(  # noqa: PLR0913
        self,
        agent_host_client: AgentHostClient | None = None,
        agent_store: AgentStore | None = None,
        kernel_config_store: KernelConfigStore | None = None,
        gateway_store: GatewayStore | None = None,
        connection_store: ConnectionStore | None = None,
        session_store: SessionStore | None = None,
        workspace_store: WorkspaceStore | None = None,
    ) -> None:
        self._agent_host = agent_host_client or HttpAgentHostClient()
        self._agent_store: AgentStore = agent_store or InMemoryAgentStore()
        self._kernel_config_store: KernelConfigStore = (
            kernel_config_store or InMemoryKernelConfigStore()
        )
        self._gateway_store: GatewayStore = gateway_store or InMemoryGatewayStore()
        self._connection_store: ConnectionStore = (
            connection_store or InMemoryConnectionStore()
        )
        self._session_store: SessionStore = session_store or InMemorySessionStore()
        self._workspace_store: WorkspaceStore = (
            workspace_store or InMemoryWorkspaceStore()
        )
        self._turns: dict[str, _ActiveTurn] = {}
        self._session_turns: dict[str, str] = {}
        self._lock = asyncio.Lock()
        self._gateway_lock = asyncio.Lock()
        self._turn_lock = asyncio.Lock()

    async def create_agent(  # noqa: PLR0913
        self,
        *,
        agent_id: str,
        name: str,
        harness: HarnessName = HarnessName.ACP,
        system_prompt: str = "",
        skills: list[str] | None = None,
        env_vars: str = "",
        connection_id: str | None = None,
        workspace_mounts: list[WorkspaceMountRecord] | None = None,
    ) -> dict[str, object]:
        _validate_agent_id(agent_id)
        if connection_id is not None:
            await self._require_connection(connection_id)
        validated_workspace_mounts = await self._validated_workspace_mounts(
            workspace_mounts or [],
        )
        agent = AgentRecord(
            agent_id=agent_id,
            name=name,
            harness=harness,
            system_prompt=system_prompt,
            skills=skills or [],
            env_vars=env_vars,
            connection_id=connection_id,
            workspace_mounts=validated_workspace_mounts,
        )
        async with self._lock:
            try:
                await self._agent_store.insert(agent)
            except AgentExistsError as exc:
                raise AgentAlreadyExistsError(agent.agent_id) from exc
        logger.info("created agent %s using harness %s", agent.agent_id, harness.value)
        return agent.summary()

    async def list_agents(self) -> list[dict[str, object]]:
        async with self._lock:
            agents = [agent.summary() for agent in await self._agent_store.list()]
        return sorted(agents, key=lambda item: str(item["created_at"]))

    async def list_harnesses(self) -> list[str]:
        return [harness.value for harness in available_harnesses()]

    async def list_kernel_configs(self) -> list[dict[str, object]]:
        records = await self._kernel_config_store.list()
        return [record.summary() for record in records]

    async def get_kernel_config(self, harness: HarnessName) -> dict[str, object]:
        record = await self._kernel_config_store.get(harness)
        if record is None:
            return {
                "harness": harness.value,
                "env_vars": "",
                "updated_at": None,
            }
        return record.summary()

    async def update_kernel_config(
        self,
        harness: HarnessName,
        env_vars: str,
    ) -> dict[str, object]:
        record = await self._kernel_config_store.upsert(harness, env_vars)
        logger.info("updated kernel config for %s", harness.value)
        return record.summary()

    async def create_workspace(
        self,
        *,
        workspace_id: str,
        name: str,
    ) -> dict[str, object]:
        _validate_workspace_id(workspace_id)
        workspace = WorkspaceRecord(workspace_id=workspace_id, name=name)
        async with self._lock:
            try:
                await self._workspace_store.insert(workspace)
            except WorkspaceExistsError as exc:
                raise WorkspaceAlreadyExistsError(workspace_id) from exc
        logger.info("created workspace %s", workspace_id)
        return workspace.summary()

    async def list_workspaces(self) -> list[dict[str, object]]:
        async with self._lock:
            workspaces = [
                workspace.summary() for workspace in await self._workspace_store.list()
            ]
        return sorted(workspaces, key=lambda item: str(item["created_at"]))

    async def get_workspace(self, workspace_id: str) -> dict[str, object]:
        workspace = await self._require_workspace(workspace_id)
        return workspace.summary()

    async def update_workspace(
        self,
        workspace_id: str,
        *,
        name: str | None | object = _UNSPECIFIED,
    ) -> dict[str, object]:
        async with self._lock:
            workspace = await self._require_workspace(workspace_id)
            if name is not _UNSPECIFIED and name is not None:
                workspace.name = str(name)
            workspace.updated_at = utc_now()
            try:
                await self._workspace_store.update(workspace)
            except WorkspaceMissingError as exc:
                raise WorkspaceNotFoundError(workspace_id) from exc
        return workspace.summary()

    async def delete_workspace(self, workspace_id: str) -> None:
        async with self._lock:
            workspace = await self._workspace_store.get(workspace_id)
            if workspace is None:
                raise WorkspaceNotFoundError(workspace_id)
            agents = await self._agent_store.list()
            in_use_by = [
                agent.agent_id
                for agent in agents
                if any(
                    mount.workspace_id == workspace_id
                    for mount in agent.workspace_mounts
                )
            ]
            if in_use_by:
                msg = (
                    f"workspace {workspace_id!r} is still mounted by agents: "
                    f"{', '.join(sorted(in_use_by))}"
                )
                raise WorkspaceInUseError(msg)
            removed = await self._workspace_store.delete(workspace_id)
        if not removed:
            raise WorkspaceNotFoundError(workspace_id)
        logger.info("deleted workspace registration %s", workspace_id)

    async def get_agent(self, agent_id: str) -> dict[str, object]:
        agent = await self._require_agent(agent_id)
        return agent.summary()

    async def update_agent(  # noqa: PLR0913
        self,
        agent_id: str,
        *,
        name: str | None | object = _UNSPECIFIED,
        harness: HarnessName | None | object = _UNSPECIFIED,
        system_prompt: str | None | object = _UNSPECIFIED,
        skills: list[str] | None | object = _UNSPECIFIED,
        env_vars: str | None | object = _UNSPECIFIED,
        connection_id: str | None | object = _UNSPECIFIED,
        workspace_mounts: list[WorkspaceMountRecord] | None | object = _UNSPECIFIED,
    ) -> dict[str, object]:
        async with self._lock:
            agent = await self._require_agent(agent_id)
            if name is not _UNSPECIFIED and name is not None:
                agent.name = str(name)
            if harness is not _UNSPECIFIED and harness is not None:
                agent.harness = harness  # type: ignore[assignment]
            if system_prompt is not _UNSPECIFIED and system_prompt is not None:
                agent.system_prompt = str(system_prompt)
            if skills is not _UNSPECIFIED and skills is not None:
                agent.skills = list(skills)  # type: ignore[arg-type]
            if env_vars is not _UNSPECIFIED and env_vars is not None:
                agent.env_vars = str(env_vars)
            if connection_id is not _UNSPECIFIED:
                cid = connection_id  # str | None
                if isinstance(cid, str):
                    await self._require_connection(cid)
                agent.connection_id = cid  # type: ignore[assignment]
            if workspace_mounts is not _UNSPECIFIED and workspace_mounts is not None:
                agent.workspace_mounts = await self._validated_workspace_mounts(
                    list(cast("list[WorkspaceMountRecord]", workspace_mounts)),
                )
            agent.updated_at = utc_now()
            try:
                await self._agent_store.update(agent)
            except AgentMissingError as exc:
                raise AgentNotFoundError(agent_id) from exc
            return agent.summary()

    async def delete_agent(self, agent_id: str) -> None:
        async with self._lock:
            removed = await self._agent_store.delete(agent_id)
            session_ids = [
                session.session_id
                for session in await self._session_store.list()
                if session.agent_id == agent_id
            ]
        if not removed:
            raise AgentNotFoundError(agent_id)
        for session_id in session_ids:
            await self.delete_session(session_id)

    async def create_session(
        self,
        *,
        agent_id: str,
        channel_name: str | None = None,
        client_type: ClientType | None = None,
    ) -> dict[str, object]:
        agent = await self._require_agent(agent_id)
        kernel_config = await self._kernel_config_store.get(agent.harness)
        env: dict[str, str] = {}
        if kernel_config is not None:
            env.update(parse_env_vars(kernel_config.env_vars))
        if agent.connection_id is not None:
            connection = await self._require_connection(agent.connection_id)
            env["CONNECTION_URL"] = connection.url
            env["CONNECTION_API_FLAVOR"] = connection.api_flavor
            if connection.api_key:
                env["CONNECTION_API_KEY"] = connection.api_key
        env.update(parse_env_vars(agent.env_vars))
        if agent.system_prompt:
            env["KERNEL_SYSTEM_PROMPT"] = agent.system_prompt
        upstream = await self._agent_host.create_session(
            harness=agent.harness,
            skills=agent.skills,
            env=env,
            workspace_mounts=[mount.summary() for mount in agent.workspace_mounts],
        )
        session = SessionRecord(
            session_id=uuid.uuid4().hex,
            agent_id=agent_id,
            agent_host_session_id=str(upstream["session_id"]),
            status=str(upstream["status"]),
            channel_name=channel_name,
            client_type=client_type,
        )
        async with self._lock:
            await self._session_store.insert(session)
        logger.info(
            "created client session %s -> agent_host %s",
            session.session_id,
            session.agent_host_session_id,
        )
        return session.summary()

    async def list_sessions(self) -> list[dict[str, object]]:
        sessions = [
            self._session_summary(session)
            for session in await self._session_store.list()
        ]
        return sorted(sessions, key=lambda item: str(item["created_at"]))

    async def get_session(self, session_id: str) -> dict[str, object]:
        session = await self._get_session(session_id)
        upstream = await self._agent_host.get_session(session.agent_host_session_id)
        session.status = str(upstream["status"])
        session.updated_at = utc_now()
        await self._session_store.update(session)
        return self._session_detail(session)

    async def list_messages(self, session_id: str) -> list[dict[str, object]]:
        session = await self._get_session(session_id)
        return [message.summary() for message in session.messages]

    async def send_message(self, session_id: str, message: str) -> dict[str, object]:
        turn = await self._create_turn(session_id, message)
        self._ensure_turn_task(turn)
        return await self._accumulate_stream(self._stream_existing_turn(turn))

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[dict[str, object]]:
        async def iterator() -> AsyncIterator[dict[str, object]]:
            turn = await self._create_turn(session_id, message)
            async for chunk in self._stream_existing_turn(turn, start=True):
                yield chunk

        return iterator()

    def stream_turn(
        self,
        session_id: str,
        turn_id: str,
    ) -> AsyncIterator[dict[str, object]]:
        async def iterator() -> AsyncIterator[dict[str, object]]:
            turn = await self._get_turn(session_id, turn_id)
            async for chunk in self._stream_existing_turn(turn, start=True):
                yield chunk

        return iterator()

    async def list_kernels(self) -> list[dict[str, object]]:
        upstream_sessions = await self._agent_host.list_sessions(with_stats=True)
        sessions = await self._session_store.list()
        kernels: list[dict[str, object]] = []
        for upstream in upstream_sessions:
            agent_host_session_id = str(upstream["session_id"])
            client_sessions = [
                session
                for session in sessions
                if session.agent_host_session_id == agent_host_session_id
            ]
            client_session_ids = [session.session_id for session in client_sessions]
            agent_ids = sorted({session.agent_id for session in client_sessions})
            channel_names = sorted(
                {
                    session.channel_name
                    for session in client_sessions
                    if session.channel_name is not None
                },
            )
            kernels.append(
                {
                    **upstream,
                    "client_session_ids": client_session_ids,
                    "channel_names": channel_names,
                    "agent_ids": agent_ids,
                },
            )
        return kernels

    async def kill_kernel(self, kernel_session_id: str) -> None:
        upstream_sessions = await self._agent_host.list_sessions()
        found = any(
            str(s["session_id"]) == kernel_session_id for s in upstream_sessions
        )
        if not found:
            raise KernelNotFoundError(kernel_session_id)
        await self._agent_host.destroy_session(kernel_session_id)
        async with self._lock:
            affected = [
                session
                for session in await self._session_store.list()
                if session.agent_host_session_id == kernel_session_id
            ]
            for session in affected:
                session.status = "dead"
                session.updated_at = utc_now()
                await self._session_store.update(session)
        logger.info(
            "killed kernel %s, marked %d client sessions as dead",
            kernel_session_id,
            len(affected),
        )

    async def kernel_logs(self, kernel_session_id: str) -> list[str]:
        upstream_sessions = await self._agent_host.list_sessions()
        found = any(
            str(s["session_id"]) == kernel_session_id for s in upstream_sessions
        )
        if not found:
            raise KernelNotFoundError(kernel_session_id)
        return await self._agent_host.logs(kernel_session_id)

    async def kernel_container_logs(
        self,
        kernel_session_id: str,
        *,
        tail: int | None,
    ) -> list[str]:
        upstream_sessions = await self._agent_host.list_sessions()
        found = any(
            str(s["session_id"]) == kernel_session_id for s in upstream_sessions
        )
        if not found:
            raise KernelNotFoundError(kernel_session_id)
        return await self._agent_host.container_logs(
            kernel_session_id,
            tail=tail,
        )

    # --- Skills (proxied to agent_host) ---

    async def create_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> dict[str, object]:
        return await self._agent_host.create_skill(skill_id, files)

    async def get_skill(self, skill_id: str) -> dict[str, object]:
        return await self._agent_host.get_skill(skill_id)

    async def list_skills(self) -> list[dict[str, object]]:
        return await self._agent_host.list_skills()

    async def update_skill(
        self,
        skill_id: str,
        files: dict[str, str],
    ) -> dict[str, object]:
        return await self._agent_host.update_skill(skill_id, files)

    async def delete_skill(self, skill_id: str) -> None:
        await self._agent_host.delete_skill(skill_id)

    async def info(self) -> dict[str, object]:
        client_env = {
            key: value
            for key, value in os.environ.items()
            if key.startswith(CLIENT_SERVICE_ENV_PREFIX)
        }
        client_section: dict[str, object] = {
            "service": "client_service",
            "env_prefix": CLIENT_SERVICE_ENV_PREFIX,
            "env": client_env,
        }

        agent_host_section: dict[str, object]
        try:
            agent_host_section = await self._agent_host.info()
        except Exception as exc:  # noqa: BLE001 - /info should degrade gracefully
            logger.warning("failed to fetch agent_host info: %s", exc)
            agent_host_section = {"service": "agent_host", "error": str(exc)}

        return {
            "client_service": client_section,
            "agent_host": agent_host_section,
        }

    async def _create_turn(self, session_id: str, message: str) -> _ActiveTurn:
        session = await self._get_session(session_id)
        async with self._turn_lock:
            existing_turn_id = self._session_turns.get(session_id)
            if existing_turn_id is not None:
                return self._turns[existing_turn_id]
            user_message = MessageRecord(
                message_id=uuid.uuid4().hex,
                session_id=session.session_id,
                role=MessageRole.USER,
                content=message,
            )
            assistant_message = MessageRecord(
                message_id=uuid.uuid4().hex,
                session_id=session.session_id,
                role=MessageRole.ASSISTANT,
                content="",
            )
            await self._session_store.append_message(user_message)
            await self._session_store.append_message(assistant_message)
            session.status = "busy"
            session.updated_at = utc_now()
            await self._session_store.update(session)
            turn = _ActiveTurn(
                turn_id=uuid.uuid4().hex,
                session_id=session.session_id,
                agent_host_session_id=session.agent_host_session_id,
                message=message,
                user_message=user_message,
                assistant_message=assistant_message,
            )
            self._turns[turn.turn_id] = turn
            self._session_turns[session.session_id] = turn.turn_id
            return turn

    async def _get_turn(self, session_id: str, turn_id: str) -> _ActiveTurn:
        async with self._turn_lock:
            turn = self._turns.get(turn_id)
            if turn is None or turn.session_id != session_id:
                msg = f"turn not found: {turn_id}"
                raise SessionNotFoundError(msg)
            return turn

    def _ensure_turn_task(self, turn: _ActiveTurn) -> None:
        if turn.task is not None:
            return
        turn.task = asyncio.create_task(
            self._run_turn(turn),
            name=f"client-turn-{turn.turn_id[:12]}",
        )

    async def _stream_existing_turn(
        self,
        turn: _ActiveTurn,
        *,
        start: bool = False,
    ) -> AsyncIterator[dict[str, object]]:
        queue: asyncio.Queue[dict[str, object] | None] = asyncio.Queue()
        async with self._turn_lock:
            if turn.final_payload is not None:
                await queue.put(turn.final_payload)
                await queue.put(None)
            else:
                turn.subscribers.add(queue)
        if start:
            self._ensure_turn_task(turn)
        try:
            while True:
                item = await queue.get()
                if item is None:
                    return
                yield item
        finally:
            async with self._turn_lock:
                turn.subscribers.discard(queue)

    async def _run_turn(self, turn: _ActiveTurn) -> None:
        started_at = perf_counter()
        logger.info(
            "client stream start: client_session=%s upstream_session=%s chars=%d",
            turn.session_id,
            turn.agent_host_session_id,
            len(turn.message),
        )
        completed = False
        stream = self._agent_host.stream_message(
            turn.agent_host_session_id,
            turn.message,
        )
        try:
            async for event in stream:
                if not turn.events:
                    logger.info(
                        "client first event: session=%s elapsed_ms=%.1f type=%s",
                        turn.session_id,
                        (perf_counter() - started_at) * 1000,
                        event.type,
                    )
                turn.events.append(event)
                await self._persist_turn_assistant(turn)
                await self._broadcast_turn(
                    turn,
                    {"type": "event", "event": asdict(event)},
                )
            completed = True
        except Exception as exc:
            turn.error = str(exc)
            logger.exception("client turn failed: session=%s", turn.session_id)
        finally:
            aclose = getattr(stream, "aclose", None)
            if callable(aclose):
                await cast("AcloseFn", aclose)()
            await self._finalize_turn(turn, completed=completed)

        logger.info(
            "client stream final: session=%s elapsed_ms=%.1f events=%d completed=%s",
            turn.session_id,
            (perf_counter() - started_at) * 1000,
            len(turn.events),
            completed,
        )

    async def _persist_turn_assistant(self, turn: _ActiveTurn) -> None:
        if not _has_visible_assistant_events(turn.events):
            return
        turn.assistant_message = _build_assistant_message(
            turn.session_id,
            turn.events,
            message_id=turn.assistant_message.message_id,
            created_at=turn.assistant_message.created_at,
        )
        await self._session_store.update_message(turn.assistant_message)

    async def _finalize_turn(self, turn: _ActiveTurn, *, completed: bool) -> None:
        await self._persist_turn_assistant(turn)
        session = await self._get_session(turn.session_id)
        try:
            upstream = await self._agent_host.get_session(turn.agent_host_session_id)
            session.status = str(upstream["status"])
        except Exception:
            logger.exception("failed to refresh upstream session %s", turn.session_id)
            session.status = "error" if turn.error else session.status
        session.updated_at = utc_now()
        await self._session_store.update(session)
        async with self._turn_lock:
            if self._session_turns.get(turn.session_id) == turn.turn_id:
                self._session_turns.pop(turn.session_id, None)
            self._turns.pop(turn.turn_id, None)
        payload: dict[str, object] = {
            "type": "final",
            "session": self._session_summary(session),
            "assistant_message": turn.assistant_message.summary(),
            "events": [asdict(event) for event in turn.events],
            "turn_id": turn.turn_id,
            "completed": completed,
        }
        if turn.error is not None:
            payload["error"] = turn.error
        turn.final_payload = payload
        await self._broadcast_turn(turn, payload, close=True)

    async def _broadcast_turn(
        self,
        turn: _ActiveTurn,
        item: dict[str, object],
        *,
        close: bool = False,
    ) -> None:
        subscribers = list(turn.subscribers)
        for queue in subscribers:
            await queue.put(item)
            if close:
                await queue.put(None)

    async def _accumulate_stream(
        self,
        stream: AsyncIterator[dict[str, object]],
    ) -> dict[str, object]:
        final_payload: dict[str, object] | None = None
        async for item in stream:
            if item.get("type") == "final":
                final_payload = item
        if final_payload is None:
            msg = "message stream ended without a final payload"
            raise RuntimeError(msg)
        return {key: value for key, value in final_payload.items() if key != "type"}

    async def reset_session(self, session_id: str) -> dict[str, object]:
        await self._cancel_active_turn(session_id)
        session = await self._get_session(session_id)
        upstream = await self._agent_host.reset_session(session.agent_host_session_id)
        session.agent_host_session_id = str(upstream["session_id"])
        session.status = str(upstream["status"])
        await self._session_store.clear_messages(session_id)
        session.updated_at = utc_now()
        await self._session_store.update(session)
        logger.info("reset client session %s", session_id)
        return self._session_summary(session)

    async def delete_session(self, session_id: str) -> None:
        await self._cancel_active_turn(session_id)
        session = await self._get_session(session_id)
        removed = await self._session_store.delete(session_id)
        if not removed:
            raise SessionNotFoundError(session_id)
        await self._agent_host.destroy_session(session.agent_host_session_id)

    async def _require_agent(self, agent_id: str) -> AgentRecord:
        agent = await self._agent_store.get(agent_id)
        if agent is None:
            raise AgentNotFoundError(agent_id)
        return agent

    async def _require_workspace(self, workspace_id: str) -> WorkspaceRecord:
        workspace = await self._workspace_store.get(workspace_id)
        if workspace is None:
            raise WorkspaceNotFoundError(workspace_id)
        return workspace

    async def _validated_workspace_mounts(
        self,
        mounts: list[WorkspaceMountRecord],
    ) -> list[WorkspaceMountRecord]:
        validated: list[WorkspaceMountRecord] = []
        seen: set[str] = set()
        for mount in mounts:
            _validate_workspace_id(mount.workspace_id)
            if mount.workspace_id in seen:
                msg = f"workspace {mount.workspace_id!r} is mounted more than once"
                raise InvalidWorkspaceIdError(msg)
            await self._require_workspace(mount.workspace_id)
            validated.append(
                WorkspaceMountRecord(
                    workspace_id=mount.workspace_id,
                    mode=WorkspaceMountMode(mount.mode),
                ),
            )
            seen.add(mount.workspace_id)
        return validated

    async def _get_session(self, session_id: str) -> SessionRecord:
        session = await self._session_store.get(session_id)
        if session is None:
            raise SessionNotFoundError(session_id)
        return session

    async def _cancel_active_turn(self, session_id: str) -> None:
        async with self._turn_lock:
            turn_id = self._session_turns.pop(session_id, None)
            turn = self._turns.pop(turn_id, None) if turn_id is not None else None
        if turn is None or turn.task is None or turn.task.done():
            return
        turn.task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await turn.task

    def _session_summary(self, session: SessionRecord) -> dict[str, object]:
        data = session.summary()
        active_turn = self._active_turn_summary(session.session_id)
        if active_turn is not None:
            data["active_turn"] = active_turn
        return data

    def _session_detail(self, session: SessionRecord) -> dict[str, object]:
        data = session.detail()
        active_turn = self._active_turn_summary(session.session_id)
        if active_turn is not None:
            data["active_turn"] = active_turn
        return data

    def _active_turn_summary(self, session_id: str) -> dict[str, object] | None:
        turn_id = self._session_turns.get(session_id)
        if turn_id is None:
            return None
        turn = self._turns.get(turn_id)
        if turn is None:
            return None
        return {
            "turn_id": turn.turn_id,
            "user_message_id": turn.user_message.message_id,
            "assistant_message_id": turn.assistant_message.message_id,
            "status": "running",
        }

    async def list_gateways(
        self,
        *,
        include_secrets: bool = False,
    ) -> list[dict[str, object]]:
        records = await self._gateway_store.list()
        return [r.summary(include_secrets=include_secrets) for r in records]

    async def get_gateway(
        self,
        gateway_id: str,
        *,
        include_secrets: bool = False,
    ) -> dict[str, object]:
        record = await self._require_gateway(gateway_id)
        return record.summary(include_secrets=include_secrets)

    async def create_gateway(  # noqa: PLR0913
        self,
        *,
        gateway_id: str,
        name: str,
        gateway_type: GatewayType,
        agent_id: str,
        enabled: bool = False,
        env_vars: str = "",
        secrets: dict[str, str] | None = None,
    ) -> dict[str, object]:
        _validate_gateway_id(gateway_id)
        await self._require_agent(agent_id)
        record = GatewayRecord(
            gateway_id=gateway_id,
            name=name,
            gateway_type=gateway_type,
            agent_id=agent_id,
            enabled=enabled,
            env_vars=env_vars,
            secrets=dict(secrets or {}),
        )
        async with self._gateway_lock:
            try:
                await self._gateway_store.insert(record)
            except GatewayExistsError as exc:
                raise GatewayAlreadyExistsError(gateway_id) from exc
        if enabled:
            await self.start_gateway(gateway_id)
        logger.info("created gateway %s (%s)", gateway_id, gateway_type.value)
        return await self.get_gateway(gateway_id)

    async def update_gateway(  # noqa: PLR0913
        self,
        gateway_id: str,
        *,
        name: str | None = None,
        agent_id: str | None = None,
        enabled: bool | None = None,
        env_vars: str | None = None,
        secrets: dict[str, str] | None = None,
    ) -> dict[str, object]:
        """Update a gateway's persisted config and (if needed) restart it.

        Semantics:

        * Only fields passed as non-``None`` are touched.
        * ``secrets`` is treated as an *overlay*: keys present in the dict
          replace the corresponding stored values, but keys not in the
          dict are preserved.  This lets the WebUI edit a single secret
          without requiring the caller to re-supply every existing
          secret value (which the API never returns).  To remove a
          secret today, delete and recreate the gateway.
        * If ``enabled`` toggles, the gateway is started or stopped to
          match.
        * If a config-affecting field (``agent_id``, ``env_vars``, or
          ``secrets``) changes while the gateway is currently running,
          the container is torn down and respawned with the new config.
          Tear-down + respawn (rather than in-place reload) keeps the
          state machine simple and matches the existing start/stop
          paths.
        """
        async with self._gateway_lock:
            record = await self._require_gateway(gateway_id)
            previously_enabled = record.enabled
            was_running = record.status == "running"
            config_changed = False
            if name is not None:
                record.name = name
            if agent_id is not None and agent_id != record.agent_id:
                await self._require_agent(agent_id)
                record.agent_id = agent_id
                config_changed = True
            if enabled is not None:
                record.enabled = enabled
            if env_vars is not None and env_vars != record.env_vars:
                record.env_vars = env_vars
                config_changed = True
            if secrets is not None:
                merged = dict(record.secrets)
                merged.update(secrets)
                if merged != record.secrets:
                    record.secrets = merged
                    config_changed = True
            record.updated_at = utc_now()
            await self._gateway_store.update(record)
        # Enable/disable transitions take precedence: an "enable" implies
        # a fresh start with whatever config was just persisted, so no
        # extra restart is needed in that case.
        if enabled is True and not previously_enabled:
            await self.start_gateway(gateway_id)
        elif enabled is False and previously_enabled:
            await self.stop_gateway(gateway_id)
        elif config_changed and was_running:
            logger.info("config changed for running gateway %s; restarting", gateway_id)
            await self.stop_gateway(gateway_id)
            await self.start_gateway(gateway_id)
        return await self.get_gateway(gateway_id)

    async def delete_gateway(self, gateway_id: str) -> None:
        async with self._gateway_lock:
            record = await self._gateway_store.get(gateway_id)
            if record is None:
                raise GatewayNotFoundError(gateway_id)
            if record.status not in {"stopped", "error"}:
                try:
                    await self._agent_host.destroy_gateway(gateway_id)
                except Exception:
                    logger.exception(
                        "failed to destroy gateway container %s during "
                        "delete; the DB row will still be removed and the "
                        "container may be orphaned (manual cleanup required)",
                        gateway_id,
                    )
            await self._gateway_store.delete(gateway_id)
        logger.info("deleted gateway %s", gateway_id)

    async def start_gateway(self, gateway_id: str) -> dict[str, object]:
        async with self._gateway_lock:
            return await self._start_gateway_locked(gateway_id)

    async def _start_gateway_locked(self, gateway_id: str) -> dict[str, object]:
        record = await self._require_gateway(gateway_id)
        env = parse_env_vars(record.env_vars)
        env.update(record.secrets)
        record.status = "starting"
        record.last_error = None
        record.updated_at = utc_now()
        await self._gateway_store.update(record)
        try:
            response = await self._agent_host.create_gateway(
                gateway_id=gateway_id,
                gateway_type=record.gateway_type.value,
                agent_id=record.agent_id,
                env=env,
            )
        except Exception as exc:
            record.status = "error"
            record.last_error = str(exc)
            record.updated_at = utc_now()
            await self._gateway_store.update(record)
            logger.exception("failed to start gateway %s", gateway_id)
            raise
        record.status = "running"
        record.container_name = (
            str(response.get("container_name"))
            if response.get("container_name")
            else None
        )
        record.updated_at = utc_now()
        await self._gateway_store.update(record)
        logger.info("started gateway %s", gateway_id)
        return record.summary()

    async def stop_gateway(self, gateway_id: str) -> dict[str, object]:
        async with self._gateway_lock:
            return await self._stop_gateway_locked(gateway_id)

    async def _stop_gateway_locked(self, gateway_id: str) -> dict[str, object]:
        record = await self._require_gateway(gateway_id)
        try:
            await self._agent_host.destroy_gateway(gateway_id)
        except Exception as exc:
            record.last_error = str(exc)
            logger.exception("error while stopping gateway %s", gateway_id)
        record.status = "stopped"
        record.container_name = None
        record.updated_at = utc_now()
        await self._gateway_store.update(record)
        return record.summary()

    async def gateway_logs(self, gateway_id: str) -> list[str]:
        await self._require_gateway(gateway_id)
        return await self._agent_host.gateway_logs(gateway_id)

    async def autostart_enabled_gateways(
        self,
        *,
        max_attempts: int = 5,
        initial_backoff: float = 1.0,
        max_backoff: float = 30.0,
    ) -> None:
        """Start every gateway flagged ``enabled``, with bounded retry.

        Intended to be scheduled as a background task during application
        startup so it does not block ``lifespan`` from yielding.  Each
        gateway is retried independently; failures are logged but do not
        abort the loop.

        Before starting anything, persisted gateway statuses are
        reconciled against the live state reported by ``agent_host``.
        ``client_service`` may have been restarted while gateway records
        in the DB still say ``running`` from a previous run; without this
        reconcile the UI would show stale "Stop" buttons for gateways
        whose containers no longer exist.
        """
        await self._reconcile_gateway_statuses()
        for record in await self._gateway_store.list():
            if not record.enabled:
                continue
            if record.status == "running":
                # Already running (per reconcile); nothing to autostart.
                continue
            backoff = initial_backoff
            for attempt in range(1, max_attempts + 1):
                try:
                    await self.start_gateway(record.gateway_id)
                except Exception:
                    logger.exception(
                        "autostart attempt %d/%d for gateway %s failed",
                        attempt,
                        max_attempts,
                        record.gateway_id,
                    )
                    if attempt >= max_attempts:
                        break
                    await asyncio.sleep(backoff)
                    backoff = min(backoff * 2, max_backoff)
                else:
                    break

    async def _reconcile_gateway_statuses(self) -> None:
        """Sync persisted gateway status with ``agent_host``'s in-memory view.

        ``agent_host`` only knows about gateways it created in the
        current process; it does not rehydrate from Docker on startup.
        So this reconcile is authoritative only for the common case
        where ``client_service`` restarts while ``agent_host`` was up
        the whole time, or where both restarted together (in which case
        no gateway containers from a prior run are still being managed).

        If ``agent_host`` is unreachable we deliberately leave persisted
        records untouched rather than guess; flipping live records to
        ``stopped`` based on a transient network failure would be more
        confusing than leaving stale state for the user to clear.
        """
        try:
            live = await self._agent_host.list_gateways()
        except Exception:
            logger.exception(
                "startup reconcile: failed to list gateways from agent_host; "
                "leaving persisted gateway statuses untouched",
            )
            return
        live_containers: dict[str, str | None] = {}
        for item in live:
            gid = item.get("gateway_id")
            if not isinstance(gid, str):
                continue
            container = item.get("container_name")
            live_containers[gid] = container if isinstance(container, str) else None
        async with self._gateway_lock:
            for record in await self._gateway_store.list():
                if record.gateway_id in live_containers:
                    new_status = "running"
                    new_container = live_containers[record.gateway_id]
                else:
                    new_status = "stopped"
                    new_container = None
                if (
                    record.status == new_status
                    and record.container_name == new_container
                ):
                    continue
                record.status = new_status
                record.container_name = new_container
                record.updated_at = utc_now()
                await self._gateway_store.update(record)

    async def _require_gateway(self, gateway_id: str) -> GatewayRecord:
        record = await self._gateway_store.get(gateway_id)
        if record is None:
            raise GatewayNotFoundError(gateway_id)
        return record

    async def list_connections(self) -> list[dict[str, object]]:
        records = await self._connection_store.list()
        return [r.summary() for r in records]

    async def get_connection(
        self,
        connection_id: str,
        *,
        include_api_key: bool = False,
    ) -> dict[str, object]:
        record = await self._require_connection(connection_id)
        return record.summary(include_api_key=include_api_key)

    async def list_connection_models(self, connection_id: str) -> dict[str, object]:
        record = await self._require_connection(connection_id)
        url = record.url.rstrip("/") + "/models"
        headers: dict[str, str] = {}
        if record.api_key:
            headers["Authorization"] = f"Bearer {record.api_key}"
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                response = await client.get(url, headers=headers)
                response.raise_for_status()
                payload = response.json()
        except httpx.HTTPError as exc:
            msg = f"failed to fetch models for connection {connection_id}: {exc}"
            raise ConnectionModelsError(msg) from exc
        except ValueError as exc:
            msg = f"models response for connection {connection_id} was not valid JSON"
            raise ConnectionModelsError(msg) from exc
        if not isinstance(payload, dict):
            msg = (
                f"models response for connection {connection_id} was not a JSON object"
            )
            raise ConnectionModelsError(msg)
        return cast("dict[str, object]", payload)

    async def create_connection(
        self,
        *,
        connection_id: str,
        name: str,
        url: str,
        api_flavor: ConnectionApiFlavor = DEFAULT_CONNECTION_API_FLAVOR,
        api_key: str = "",
    ) -> dict[str, object]:
        _validate_connection_id(connection_id)
        record = ConnectionRecord(
            connection_id=connection_id,
            name=name,
            url=url,
            api_flavor=api_flavor,
            api_key=api_key,
        )
        try:
            await self._connection_store.insert(record)
        except ConnectionExistsError as exc:
            raise ConnectionAlreadyExistsError(connection_id) from exc
        logger.info("created connection %s", connection_id)
        return await self.get_connection(connection_id, include_api_key=True)

    async def update_connection(
        self,
        connection_id: str,
        *,
        name: str | None = None,
        url: str | None = None,
        api_flavor: ConnectionApiFlavor | None = None,
        api_key: str | None = None,
    ) -> dict[str, object]:
        record = await self._require_connection(connection_id)
        if name is not None:
            record.name = name
        if url is not None:
            record.url = url
        if api_flavor is not None:
            record.api_flavor = api_flavor
        if api_key is not None:
            record.api_key = api_key
        record.updated_at = utc_now()
        try:
            await self._connection_store.update(record)
        except ConnectionMissingError as exc:
            raise ConnectionNotFoundError(connection_id) from exc
        logger.info("updated connection %s", connection_id)
        return await self.get_connection(connection_id, include_api_key=True)

    async def delete_connection(self, connection_id: str) -> None:
        removed = await self._connection_store.delete(connection_id)
        if not removed:
            raise ConnectionNotFoundError(connection_id)
        logger.info("deleted connection %s", connection_id)

    async def _require_connection(
        self,
        connection_id: str,
    ) -> ConnectionRecord:
        record = await self._connection_store.get(connection_id)
        if record is None:
            raise ConnectionNotFoundError(connection_id)
        return record


def _flatten_text(events: list[KernelEvent]) -> str:
    chunks: list[str] = []
    for event in events:
        if event.type == EventType.TEXT_DELTA and event.content:
            chunks.append(event.content)
            continue
        update = _session_update(event)
        if update.get("sessionUpdate") == "agent_message_chunk":
            chunks.append(_content_text(update.get("content")))
    return "".join(chunks).strip()


def _flatten_reasoning(events: list[KernelEvent]) -> str:
    chunks: list[str] = []
    for event in events:
        if event.type == EventType.REASONING_DELTA and event.content:
            chunks.append(event.content)
            continue
        update = _session_update(event)
        update_type = update.get("sessionUpdate")
        if update_type == "agent_thought_chunk":
            chunks.append(_content_text(update.get("content")))
        elif update_type == "plan":
            chunks.append(json.dumps({"plan": update.get("entries")}, indent=2))
    return "".join(chunks).strip()


def _extract_tool_calls(events: list[KernelEvent]) -> list[ToolCallRecord]:
    calls: list[ToolCallRecord] = []
    by_id: dict[str, int] = {}
    content = ""
    for event in events:
        if event.type == EventType.TEXT_DELTA and event.content:
            content = f"{content}{event.content}"
            continue
        if event.type == EventType.TOOL_CALL and event.tool:
            calls.append(
                ToolCallRecord(
                    tool=event.tool,
                    input=_json_string(event.input),
                    content_offset=len(content.strip()),
                ),
            )
            continue
        if event.type == EventType.TOOL_RESULT and event.tool:
            _apply_legacy_tool_result(calls, event)
            continue
        update = _session_update(event)
        update_type = update.get("sessionUpdate")
        if update_type == "agent_message_chunk":
            content = f"{content}{_content_text(update.get('content'))}"
        elif update_type in {"tool_call", "tool_call_update"}:
            _upsert_tool_call(calls, by_id, update, len(content.strip()))
    return calls


def _apply_legacy_tool_result(
    calls: list[ToolCallRecord],
    event: KernelEvent,
) -> None:
    for idx, call in enumerate(calls):
        if call.tool == event.tool and call.output is None:
            calls[idx] = replace(call, output=event.output)
            return


def _session_update(event: KernelEvent) -> dict[str, object]:
    if event.type != EventType.SESSION_UPDATE or event.update is None:
        return {}
    return event.update


def _upsert_tool_call(
    calls: list[ToolCallRecord],
    by_id: dict[str, int],
    update: dict[str, object],
    content_offset: int,
) -> None:
    tool_call_id = _optional_str(update.get("toolCallId"))
    index = by_id.get(tool_call_id) if tool_call_id is not None else None
    if index is None:
        title = _optional_str(update.get("title")) or tool_call_id or "tool"
        index = len(calls)
        calls.append(
            ToolCallRecord(
                tool=title,
                tool_call_id=tool_call_id,
                content_offset=content_offset,
            ),
        )
        if tool_call_id is not None:
            by_id[tool_call_id] = index

    existing = calls[index]
    title = _optional_str(update.get("title")) or existing.tool
    status = _optional_str(update.get("status")) or existing.status
    kind = _optional_str(update.get("kind")) or existing.kind
    tool_input = existing.input
    if "rawInput" in update:
        tool_input = _json_string(update.get("rawInput"))
    output = _tool_output(update) or existing.output
    calls[index] = replace(
        existing,
        tool=title,
        status=status,
        kind=kind,
        input=tool_input,
        output=output,
    )


def _tool_output(update: dict[str, object]) -> str | None:
    if "rawOutput" in update:
        return _json_string(update.get("rawOutput"))
    content = _content_text(update.get("content"))
    return content or None


def _json_string(value: object) -> str | None:
    if value is None:
        return None
    if isinstance(value, str):
        return value
    return json.dumps(value, indent=2)


def _content_text(content: object) -> str:
    if isinstance(content, list):
        return "".join(_content_text(item) for item in cast("list[object]", content))
    if not isinstance(content, dict):
        return "" if content is None else str(content)

    content_dict = cast("dict[str, object]", content)
    content_type = content_dict.get("type")
    if content_type == "text":
        text = content_dict.get("text")
        return text if isinstance(text, str) else ""
    if content_type == "content":
        return _content_text(content_dict.get("content"))
    return json.dumps(content_dict, separators=(",", ":"))


def _optional_str(value: object) -> str | None:
    return value if isinstance(value, str) else None


def _build_assistant_message(
    session_id: str,
    events: list[KernelEvent],
    *,
    message_id: str | None = None,
    created_at: str | None = None,
) -> MessageRecord:
    return MessageRecord(
        message_id=message_id or uuid.uuid4().hex,
        session_id=session_id,
        role=MessageRole.ASSISTANT,
        content=_flatten_text(events),
        created_at=created_at or utc_now(),
        tool_calls=_extract_tool_calls(events),
        reasoning=_flatten_reasoning(events),
    )


def _has_visible_assistant_events(events: list[KernelEvent]) -> bool:
    return any(event.type in VISIBLE_ASSISTANT_EVENT_TYPES for event in events)


def _validate_agent_id(agent_id: str) -> None:
    if not AGENT_ID_PATTERN.fullmatch(agent_id):
        msg = "agent_id must use lowercase letters and single dashes only"
        raise InvalidAgentIdError(msg)


def _validate_connection_id(connection_id: str) -> None:
    if not CONNECTION_ID_PATTERN.fullmatch(connection_id):
        msg = "connection_id must use lowercase letters and single dashes only"
        raise InvalidConnectionIdError(msg)


def _validate_gateway_id(gateway_id: str) -> None:
    if not GATEWAY_ID_PATTERN.fullmatch(gateway_id):
        msg = "gateway_id must use lowercase letters and single dashes only"
        raise InvalidGatewayIdError(msg)


def _validate_workspace_id(workspace_id: str) -> None:
    if not WORKSPACE_ID_PATTERN.fullmatch(workspace_id):
        msg = "workspace_id must use lowercase letters, digits, and single dashes only"
        raise InvalidWorkspaceIdError(msg)


def parse_env_vars(raw: str) -> dict[str, str]:
    """Parse .env file content into a dict of environment variables.

    Supports KEY=VALUE lines.  Blank lines and lines starting with ``#``
    are ignored.  Values may optionally be wrapped in single or double
    quotes, which are stripped.
    """
    env: dict[str, str] = {}
    for line in raw.splitlines():
        line = line.strip()  # noqa: PLW2901
        if not line or line.startswith("#"):
            continue
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip()
        if not key:
            continue
        if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
            value = value[1:-1]
        env[key] = value
    return env
