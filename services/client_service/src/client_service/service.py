from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import uuid
from dataclasses import asdict
from typing import TYPE_CHECKING, cast

from kernel.events import EventType, KernelEvent
from kernel_host.registry import HarnessName

from client_service.agent_host_client import AgentHostClient, HttpAgentHostClient
from client_service.models import (
    AgentRecord,
    ClientType,
    MessageRecord,
    MessageRole,
    SessionRecord,
    ToolCallRecord,
    utc_now,
)
from client_service.storage.agents import (
    AgentExistsError,
    AgentMissingError,
    AgentStore,
    InMemoryAgentStore,
)
from client_service.storage.kernel_configs import (
    InMemoryKernelConfigStore,
    KernelConfigStore,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Awaitable, Callable

    type AcloseFn = Callable[[], Awaitable[object]]

logger = logging.getLogger(__name__)
AGENT_ID_PATTERN = re.compile(r"^[a-z]+(?:-[a-z]+)*$")
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


VISIBLE_ASSISTANT_EVENT_TYPES = frozenset(
    {
        EventType.TEXT_DELTA,
        EventType.REASONING_DELTA,
        EventType.TOOL_CALL,
        EventType.TOOL_RESULT,
        EventType.ERROR,
    },
)


class ClientService:
    def __init__(
        self,
        agent_host_client: AgentHostClient | None = None,
        agent_store: AgentStore | None = None,
        kernel_config_store: KernelConfigStore | None = None,
    ) -> None:
        self._agent_host = agent_host_client or HttpAgentHostClient()
        self._agent_store: AgentStore = agent_store or InMemoryAgentStore()
        self._kernel_config_store: KernelConfigStore = (
            kernel_config_store or InMemoryKernelConfigStore()
        )
        self._sessions: dict[str, SessionRecord] = {}
        self._lock = asyncio.Lock()

    async def create_agent(
        self,
        *,
        agent_id: str,
        name: str,
        harness: HarnessName = HarnessName.COPILOT_CLI,
        system_prompt: str = "",
        skills: list[str] | None = None,
        env_vars: str = "",
    ) -> dict[str, object]:
        _validate_agent_id(agent_id)
        agent = AgentRecord(
            agent_id=agent_id,
            name=name,
            harness=harness,
            system_prompt=system_prompt,
            skills=skills or [],
            env_vars=env_vars,
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
        return [harness.value for harness in HarnessName]

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

    async def get_agent(self, agent_id: str) -> dict[str, object]:
        agent = await self._require_agent(agent_id)
        return agent.summary()

    async def update_agent(
        self,
        agent_id: str,
        *,
        name: str | None,
        harness: HarnessName | None,
        system_prompt: str | None,
        skills: list[str] | None,
        env_vars: str | None,
    ) -> dict[str, object]:
        async with self._lock:
            agent = await self._require_agent(agent_id)
            if name is not None:
                agent.name = name
            if harness is not None:
                agent.harness = harness
            if system_prompt is not None:
                agent.system_prompt = system_prompt
            if skills is not None:
                agent.skills = list(skills)
            if env_vars is not None:
                agent.env_vars = env_vars
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
                session_id
                for session_id, session in self._sessions.items()
                if session.agent_id == agent_id
            ]
        if not removed:
            raise AgentNotFoundError(agent_id)
        for session_id in session_ids:
            if session_id in self._sessions:
                await self.delete_session(session_id)

    async def create_session(
        self,
        *,
        agent_id: str,
        channel_name: str | None = None,
        client_type: ClientType | None = None,
    ) -> dict[str, object]:
        agent = await self._require_agent(agent_id)
        env = parse_env_vars(agent.env_vars)
        upstream = await self._agent_host.create_session(
            harness=agent.harness,
            skills=agent.skills,
            env=env,
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
            self._sessions[session.session_id] = session
        logger.info(
            "created client session %s -> agent_host %s",
            session.session_id,
            session.agent_host_session_id,
        )
        return session.summary()

    async def list_sessions(self) -> list[dict[str, object]]:
        async with self._lock:
            sessions = [session.summary() for session in self._sessions.values()]
        return sorted(sessions, key=lambda item: str(item["created_at"]))

    async def get_session(self, session_id: str) -> dict[str, object]:
        session = self._get_session(session_id)
        upstream = await self._agent_host.get_session(session.agent_host_session_id)
        session.status = str(upstream["status"])
        session.updated_at = utc_now()
        return session.detail()

    async def list_messages(self, session_id: str) -> list[dict[str, object]]:
        session = self._get_session(session_id)
        return [message.summary() for message in session.messages]

    async def send_message(self, session_id: str, message: str) -> dict[str, object]:
        return await self._accumulate_stream(self.stream_message(session_id, message))

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[dict[str, object]]:
        session = self._get_session(session_id)
        return self._stream_to_session(session, message)

    async def list_kernels(self) -> list[dict[str, object]]:
        upstream_sessions = await self._agent_host.list_sessions()
        kernels: list[dict[str, object]] = []
        for upstream in upstream_sessions:
            agent_host_session_id = str(upstream["session_id"])
            client_sessions = [
                session
                for session in self._sessions.values()
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
                sid
                for sid, session in self._sessions.items()
                if session.agent_host_session_id == kernel_session_id
            ]
            for sid in affected:
                self._sessions[sid].status = "dead"
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

    async def _send_to_session(
        self,
        session: SessionRecord,
        message: str,
    ) -> dict[str, object]:
        return await self._accumulate_stream(self._stream_to_session(session, message))

    async def _stream_to_session(
        self,
        session: SessionRecord,
        message: str,
    ) -> AsyncIterator[dict[str, object]]:
        user_message = MessageRecord(
            message_id=uuid.uuid4().hex,
            session_id=session.session_id,
            role=MessageRole.USER,
            content=message,
        )
        session.messages.append(user_message)
        events: list[KernelEvent] = []
        assistant_message: MessageRecord | None = None
        completed = False
        stream = self._agent_host.stream_message(
            session.agent_host_session_id,
            message,
        )
        try:
            async for event in stream:
                events.append(event)
                yield {"type": "event", "event": asdict(event)}
            completed = True
        finally:
            aclose = getattr(stream, "aclose", None)
            if callable(aclose):
                await cast("AcloseFn", aclose)()
            assistant_message = await self._finalize_stream_turn(
                session=session,
                events=events,
                completed=completed,
            )

        if assistant_message is None:
            assistant_message = _build_assistant_message(session.session_id, events)

        yield {
            "type": "final",
            "session": session.summary(),
            "assistant_message": assistant_message.summary(),
            "events": [asdict(event) for event in events],
        }

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

    async def _finalize_stream_turn(
        self,
        *,
        session: SessionRecord,
        events: list[KernelEvent],
        completed: bool,
    ) -> MessageRecord | None:
        assistant_message: MessageRecord | None = None
        if events and (completed or _has_visible_assistant_events(events)):
            assistant_message = _build_assistant_message(session.session_id, events)
            session.messages.append(assistant_message)
            session.updated_at = assistant_message.created_at
        else:
            session.updated_at = utc_now()

        upstream = await self._agent_host.get_session(session.agent_host_session_id)
        session.status = str(upstream["status"])
        logger.info("stored turn for client session %s", session.session_id)
        return assistant_message

    async def reset_session(self, session_id: str) -> dict[str, object]:
        session = self._get_session(session_id)
        upstream = await self._agent_host.reset_session(session.agent_host_session_id)
        session.agent_host_session_id = str(upstream["session_id"])
        session.status = str(upstream["status"])
        session.messages.clear()
        session.updated_at = utc_now()
        logger.info("reset client session %s", session_id)
        return session.summary()

    async def delete_session(self, session_id: str) -> None:
        async with self._lock:
            session = self._sessions.pop(session_id, None)
        if session is None:
            raise SessionNotFoundError(session_id)
        await self._agent_host.destroy_session(session.agent_host_session_id)

    async def _require_agent(self, agent_id: str) -> AgentRecord:
        agent = await self._agent_store.get(agent_id)
        if agent is None:
            raise AgentNotFoundError(agent_id)
        return agent

    def _get_session(self, session_id: str) -> SessionRecord:
        try:
            return self._sessions[session_id]
        except KeyError as exc:
            raise SessionNotFoundError(session_id) from exc


def _flatten_text(events: list[KernelEvent]) -> str:
    return "".join(
        event.content or "" for event in events if event.type == EventType.TEXT_DELTA
    ).strip()


def _flatten_reasoning(events: list[KernelEvent]) -> str:
    return "".join(
        event.content or ""
        for event in events
        if event.type == EventType.REASONING_DELTA
    ).strip()


def _extract_tool_calls(events: list[KernelEvent]) -> list[ToolCallRecord]:
    """Extract tool calls with their inputs and paired outputs."""
    calls: list[ToolCallRecord] = []
    result_map: dict[str, str] = {}
    for event in events:
        if event.type == EventType.TOOL_RESULT and event.tool and event.output:
            result_map[event.tool] = event.output
    for event in events:
        if event.type == EventType.TOOL_CALL and event.tool:
            tool_input = json.dumps(event.input, indent=2) if event.input else None
            tool_output = result_map.pop(event.tool, None)
            calls.append(
                ToolCallRecord(
                    tool=event.tool,
                    input=tool_input,
                    output=tool_output,
                ),
            )
    return calls


def _build_assistant_message(
    session_id: str,
    events: list[KernelEvent],
) -> MessageRecord:
    return MessageRecord(
        message_id=uuid.uuid4().hex,
        session_id=session_id,
        role=MessageRole.ASSISTANT,
        content=_flatten_text(events),
        tool_calls=_extract_tool_calls(events),
        reasoning=_flatten_reasoning(events),
    )


def _has_visible_assistant_events(events: list[KernelEvent]) -> bool:
    return any(event.type in VISIBLE_ASSISTANT_EVENT_TYPES for event in events)


def _validate_agent_id(agent_id: str) -> None:
    if not AGENT_ID_PATTERN.fullmatch(agent_id):
        msg = "agent_id must use lowercase letters and single dashes only"
        raise InvalidAgentIdError(msg)


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
