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
    ChannelRecord,
    ChannelType,
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


class ChannelNotFoundError(KeyError):
    pass


class ClientService:
    def __init__(self, agent_host_client: AgentHostClient | None = None) -> None:
        self._agent_host = agent_host_client or HttpAgentHostClient()
        self._agents: dict[str, AgentRecord] = {}
        self._sessions: dict[str, SessionRecord] = {}
        self._channels: dict[str, ChannelRecord] = {}
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
            channel_ids = [
                channel_id
                for channel_id, channel in self._channels.items()
                if channel.agent_id == agent_id
            ]
        if agent is None:
            raise AgentNotFoundError(agent_id)
        for channel_id in channel_ids:
            if channel_id in self._channels:
                await self.delete_channel(channel_id)
        for session_id in session_ids:
            if session_id in self._sessions:
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
        return await self._send_to_session(session, message)

    async def register_channel(
        self,
        *,
        agent_id: str,
        name: str,
        channel_type: ChannelType = ChannelType.CLI,
        cwd: str | None = None,
    ) -> dict[str, str | None]:
        session_summary = await self.create_session(agent_id=agent_id, cwd=cwd)
        channel = ChannelRecord(
            channel_id=uuid.uuid4().hex,
            channel_type=channel_type,
            agent_id=agent_id,
            session_id=str(session_summary["session_id"]),
            name=name,
            cwd=cwd,
        )
        async with self._lock:
            self._channels[channel.channel_id] = channel
        logger.info(
            "registered channel %s (%s) -> session %s",
            channel.channel_id,
            channel.channel_type.value,
            channel.session_id,
        )
        return channel.summary()

    async def list_channels(self) -> list[dict[str, str | None]]:
        async with self._lock:
            channels = [channel.summary() for channel in self._channels.values()]
        return sorted(channels, key=lambda item: str(item["created_at"]))

    async def get_channel(self, channel_id: str) -> dict[str, str | None]:
        return self._get_channel(channel_id).summary()

    async def list_channel_messages(self, channel_id: str) -> list[dict[str, str]]:
        channel = self._get_channel(channel_id)
        return await self.list_messages(channel.session_id)

    async def send_channel_message(
        self,
        channel_id: str,
        message: str,
    ) -> dict[str, object]:
        channel = self._get_channel(channel_id)
        channel.updated_at = utc_now()
        session = self._get_session(channel.session_id)
        return await self._send_to_session(session, message)

    async def reset_channel(self, channel_id: str) -> dict[str, str | None]:
        channel = self._get_channel(channel_id)
        await self.reset_session(channel.session_id)
        channel.updated_at = utc_now()
        return channel.summary()

    async def delete_channel(self, channel_id: str) -> None:
        async with self._lock:
            channel = self._channels.pop(channel_id, None)
        if channel is None:
            raise ChannelNotFoundError(channel_id)
        if channel.session_id in self._sessions:
            await self.delete_session(channel.session_id)

    async def _send_to_session(
        self,
        session: SessionRecord,
        message: str,
    ) -> dict[str, object]:
        user_message = MessageRecord(
            message_id=uuid.uuid4().hex,
            session_id=session.session_id,
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
            session_id=session.session_id,
            role=MessageRole.ASSISTANT,
            content=_flatten_text(events),
        )
        session.messages.append(assistant_message)
        upstream = await self._agent_host.get_session(session.agent_host_session_id)
        session.status = str(upstream["status"])
        session.updated_at = assistant_message.created_at
        logger.info("stored turn for client session %s", session.session_id)
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

    def _get_channel(self, channel_id: str) -> ChannelRecord:
        try:
            return self._channels[channel_id]
        except KeyError as exc:
            raise ChannelNotFoundError(channel_id) from exc


def _flatten_text(events: list[KernelEvent]) -> str:
    return "".join(
        event.content or ""
        for event in events
        if event.type == EventType.TEXT_DELTA
    ).strip()
