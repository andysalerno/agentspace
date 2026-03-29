from __future__ import annotations

import os
from typing import Any

from kernel.events import EventType, KernelEvent, KernelStatus
from kernel.protocol import KernelConfig

from kernel_host.registry import HarnessName, get_kernel


class KernelSessionService:
    def __init__(
        self,
        *,
        harness: HarnessName,
        env: dict[str, str],
        cwd: str | None,
        additional_paths: tuple[str, ...],
    ) -> None:
        self._harness = harness
        self._base_env = dict(env)
        self._cwd = cwd
        self._additional_paths = additional_paths
        self._session_id: str | None = None
        self._history: list[list[KernelEvent]] = []
        self._status = KernelStatus.IDLE

    async def send_message(self, message: str) -> list[KernelEvent]:
        kernel = get_kernel(self._harness)
        config = KernelConfig(
            env=dict(self._base_env),
            cwd=self._cwd,
            session_id=self._session_id,
            additional_paths=self._additional_paths,
        )
        await kernel.start(config)
        await kernel.send(message)
        events = [event async for event in kernel.recv()]
        await kernel.stop()

        if kernel.resume_token is not None:
            self._session_id = kernel.resume_token
        self._history.append(events)
        self._status = self._derive_status(events, kernel.status)
        return events

    async def summary(self) -> dict[str, Any]:
        return {
            "harness": self._harness,
            "status": self._status,
            "turns": len(self._history),
            "resume_token": self._session_id,
            "cwd": self._cwd,
            "additional_paths": list(self._additional_paths),
        }

    async def history(self) -> list[list[KernelEvent]]:
        return list(self._history)

    async def reset(self) -> dict[str, Any]:
        self._session_id = None
        self._history.clear()
        self._status = KernelStatus.IDLE
        return await self.summary()

    async def stop(self) -> None:
        self._status = KernelStatus.DONE

    def _derive_status(
        self,
        events: list[KernelEvent],
        fallback_status: KernelStatus,
    ) -> KernelStatus:
        for event in reversed(events):
            if event.type == EventType.STATUS and event.status is not None:
                return event.status
        return fallback_status


def service_from_env() -> KernelSessionService:
    additional_paths = tuple(
        path
        for path in os.environ.get("KERNEL_ADDITIONAL_PATHS", "").split(os.pathsep)
        if path
    )
    return KernelSessionService(
        harness=HarnessName(os.environ.get("KERNEL_HARNESS", HarnessName.ECHO)),
        env=dict(os.environ),
        cwd=os.environ.get("KERNEL_WORKDIR") or None,
        additional_paths=additional_paths,
    )
