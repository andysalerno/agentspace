from __future__ import annotations

import asyncio
import logging
import os
import uuid
from dataclasses import dataclass, field, replace
from typing import TYPE_CHECKING, Any

from kernel.events import EventType, KernelEvent, KernelStatus
from kernel.protocol import KernelConfig
from kernel_host.registry import get_kernel

logger = logging.getLogger(__name__)

if TYPE_CHECKING:
    from kernel.protocol import Kernel


class SessionNotFoundError(KeyError):
    pass


def _empty_history() -> list[list[KernelEvent]]:
    return []


@dataclass(slots=True)
class SessionRecord:
    session_id: str
    harness: str
    config: KernelConfig
    kernel: Kernel
    history: list[list[KernelEvent]] = field(default_factory=_empty_history)
    status: KernelStatus = KernelStatus.IDLE

    def summary(self) -> dict[str, Any]:
        return {
            "session_id": self.session_id,
            "harness": self.harness,
            "status": self.status,
            "turns": len(self.history),
            "resume_token": self.kernel.resume_token,
        }


class AgentHost:
    def __init__(self) -> None:
        self._sessions: dict[str, SessionRecord] = {}
        self._lock = asyncio.Lock()

    async def create_session(
        self,
        *,
        harness: str = "copilot-cli",
        env: dict[str, str] | None = None,
        cwd: str | None = None,
        additional_paths: tuple[str, ...] = (),
    ) -> dict[str, Any]:
        session_id = uuid.uuid4().hex
        config_env = dict(os.environ)
        config_env.update(env or {})
        config = KernelConfig(
            env=config_env,
            cwd=cwd,
            additional_paths=additional_paths,
        )
        kernel = get_kernel(harness)
        await kernel.start(config)

        record = SessionRecord(
            session_id=session_id,
            harness=harness,
            config=config,
            kernel=kernel,
        )
        async with self._lock:
            self._sessions[session_id] = record
        logger.info("created session %s with harness=%s", session_id, harness)
        return record.summary()

    async def send_message(self, session_id: str, message: str) -> list[KernelEvent]:
        record = self._get_session(session_id)
        self._sync_resume_token(record)

        await record.kernel.send(message)
        events = [event async for event in record.kernel.recv()]
        self._sync_resume_token(record)

        record.history.append(events)
        record.status = self._derive_status(record.kernel.status, events)
        return events

    async def destroy_session(self, session_id: str) -> None:
        async with self._lock:
            record = self._sessions.pop(session_id, None)
        if record is None:
            raise SessionNotFoundError(session_id)
        await record.kernel.stop()
        logger.info("destroyed session %s", session_id)

    async def reset_session(self, session_id: str) -> dict[str, Any]:
        old_record = self._get_session(session_id)
        harness = old_record.harness
        env = dict(old_record.config.env)
        cwd = old_record.config.cwd
        additional_paths = old_record.config.additional_paths
        await self.destroy_session(session_id)
        return await self.create_session(
            harness=harness,
            env=env,
            cwd=cwd,
            additional_paths=additional_paths,
        )

    async def get_session(self, session_id: str) -> dict[str, Any]:
        return self._get_session(session_id).summary()

    async def list_sessions(self) -> list[dict[str, Any]]:
        async with self._lock:
            sessions = list(self._sessions.values())
        return [record.summary() for record in sessions]

    async def history(self, session_id: str) -> list[list[KernelEvent]]:
        return list(self._get_session(session_id).history)

    def _get_session(self, session_id: str) -> SessionRecord:
        try:
            return self._sessions[session_id]
        except KeyError as exc:
            raise SessionNotFoundError(session_id) from exc

    def _derive_status(
        self,
        fallback_status: KernelStatus,
        events: list[KernelEvent],
    ) -> KernelStatus:
        for event in reversed(events):
            if event.type == EventType.STATUS and event.status is not None:
                return event.status
        return fallback_status

    def _sync_resume_token(self, record: SessionRecord) -> None:
        resume_token = record.kernel.resume_token
        if resume_token and record.config.session_id != resume_token:
            record.config = replace(record.config, session_id=resume_token)
