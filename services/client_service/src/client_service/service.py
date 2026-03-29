from __future__ import annotations

import asyncio
import logging
import uuid
from dataclasses import asdict

from kernel.events import EventType, KernelEvent
from kernel_host.registry import HarnessName

from client_service.agent_host_client import AgentHostClient, HttpAgentHostClient
from client_service.models import (
    AgentRecord,
    MessageRecord,
    MessageRole,
    SessionRecord,
    utc_now,
)

logger = logging.getLogger(__name__)


class AgentNotFoundError(KeyError):
    pass


class SessionNotFoundError(KeyError):
    pass


class ClientService:
    def __init__(self, agent_host_client: AgentHostClient | None = None) -> None:
        self._agent_host = agent_host_client or HttpAgentHostClient()
        self._agents: dict[str, AgentRecord] = {}
        self._sessions: dict[str, SessionRecord] = {}
        self._lock = asyncio.Lock()

    async def create_agent(
        self,
        *,
        name: str,
        harness: HarnessName = HarnessName.COPILOT_CLI,
        system_prompt: str = "",
    ) -> dict[str, str]:
        agent = AgentRecord(
            agent_id=uuid.uuid4().hex,
            name=name,
            harness=harness,
            system_prompt=system_prompt,
        )
        async with self._lock:
            self._agents[agent.agent_id] = agent
        logger.info("created agent %s using harness %s", agent.agent_id, harness.value)
        return agent.summary()

    async def list_agents(self) -> list[dict[str, str]]:
        async with self._lock:
            agents = [agent.summary() for agent in self._agents.values()]
        return sorted(agents, key=lambda item: str(item["created_at"]))

    async def get_agent(self, agent_id: str) -> dict[str, str]:
        return self._get_agent(agent_id).summary()

    async def update_agent(
        self,
        agent_id: str,
        *,
        name: str | None,
        harness: HarnessName | None,
        system_prompt: str | None,
    ) -> dict[str, str]:
        agent = self._get_agent(agent_id)
        if name is not None:
            agent.name = name
        if harness is not None:
            agent.harness = harness
        if system_prompt is not None:
            agent.system_prompt = system_prompt
        agent.updated_at = utc_now()
        return agent.summary()

    async def delete_agent(self, agent_id: str) -> None:
        async with self._lock:
            agent = self._agents.pop(agent_id, None)
            session_ids = [
                session_id
                for session_id, session in self._sessions.items()
                if session.agent_id == agent_id
            ]
        if agent is None:
            raise AgentNotFoundError(agent_id)
        for session_id in session_ids:
            await self.delete_session(session_id)

    async def create_session(
        self,
        *,
        agent_id: str,
        cwd: str | None,
    ) -> dict[str, object]:
        agent = self._get_agent(agent_id)
        upstream = await self._agent_host.create_session(harness=agent.harness, cwd=cwd)
        session = SessionRecord(
            session_id=uuid.uuid4().hex,
            agent_id=agent_id,
            agent_host_session_id=str(upstream["session_id"]),
            status=str(upstream["status"]),
            cwd=cwd,
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

    async def list_messages(self, session_id: str) -> list[dict[str, str]]:
        session = self._get_session(session_id)
        return [message.summary() for message in session.messages]

    async def send_message(self, session_id: str, message: str) -> dict[str, object]:
        session = self._get_session(session_id)
        user_message = MessageRecord(
            message_id=uuid.uuid4().hex,
            session_id=session_id,
            role=MessageRole.USER,
            content=message,
        )
        session.messages.append(user_message)
        events = await self._agent_host.send_message(
            session.agent_host_session_id,
            message,
        )
        assistant_message = MessageRecord(
            message_id=uuid.uuid4().hex,
            session_id=session_id,
            role=MessageRole.ASSISTANT,
            content=_flatten_text(events),
        )
        session.messages.append(assistant_message)
        upstream = await self._agent_host.get_session(session.agent_host_session_id)
        session.status = str(upstream["status"])
        session.updated_at = assistant_message.created_at
        logger.info("stored turn for client session %s", session_id)
        return {
            "session": session.summary(),
            "assistant_message": assistant_message.summary(),
            "events": [asdict(event) for event in events],
        }

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

    def _get_agent(self, agent_id: str) -> AgentRecord:
        try:
            return self._agents[agent_id]
        except KeyError as exc:
            raise AgentNotFoundError(agent_id) from exc

    def _get_session(self, session_id: str) -> SessionRecord:
        try:
            return self._sessions[session_id]
        except KeyError as exc:
            raise SessionNotFoundError(session_id) from exc


def _flatten_text(events: list[KernelEvent]) -> str:
    return "".join(
        event.content or ""
        for event in events
        if event.type == EventType.TEXT_DELTA
    ).strip()
