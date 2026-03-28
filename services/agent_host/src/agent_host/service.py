from __future__ import annotations

import asyncio
import os
import uuid
from dataclasses import dataclass, field
from typing import Any, Protocol

import docker
import httpx
from docker.errors import NotFound
from kernel.events import EventType, KernelEvent, KernelStatus

logger_name = __name__

class SessionNotFoundError(KeyError):
    pass


class KernelRuntime(Protocol):
    async def create_session(
        self,
        *,
        session_id: str,
        harness: str,
        env: dict[str, str],
        cwd: str | None,
        additional_paths: tuple[str, ...],
    ) -> tuple[str, str]: ...

    async def send_message(
        self,
        *,
        base_url: str,
        message: str,
    ) -> list[KernelEvent]: ...

    async def summary(self, *, base_url: str) -> dict[str, Any]: ...

    async def history(self, *, base_url: str) -> list[list[KernelEvent]]: ...

    async def destroy_session(self, *, container_name: str) -> None: ...


def _empty_history() -> list[list[KernelEvent]]:
    return []


@dataclass(slots=True)
class SessionRecord:
    session_id: str
    harness: str
    container_name: str
    base_url: str
    env: dict[str, str]
    cwd: str | None
    additional_paths: tuple[str, ...]
    history: list[list[KernelEvent]] = field(default_factory=_empty_history)
    status: KernelStatus = KernelStatus.IDLE
    resume_token: str | None = None

    def summary(self) -> dict[str, Any]:
        return {
            "session_id": self.session_id,
            "harness": self.harness,
            "container_name": self.container_name,
            "status": self.status,
            "turns": len(self.history),
            "resume_token": self.resume_token,
            "cwd": self.cwd,
            "additional_paths": list(self.additional_paths),
        }


class DockerKernelRuntime:
    def __init__(self) -> None:
        self._client = docker.from_env()
        self._kernel_image = os.environ.get(
            "AGENT_HOST_KERNEL_IMAGE",
            "agentspace-kernel-kernel:latest",
        )
        self._kernel_network = os.environ.get(
            "AGENT_HOST_DOCKER_NETWORK",
            "agentspace-agent-host_default",
        )
        self._base_url_template = os.environ.get(
            "AGENT_HOST_KERNEL_BASE_URL_TEMPLATE",
            "http://{container_name}:8000",
        )
        self._startup_timeout = float(
            os.environ.get("AGENT_HOST_KERNEL_STARTUP_TIMEOUT", "60"),
        )
        self._copilot_volume = os.environ.get(
            "AGENT_HOST_COPILOT_VOLUME",
            "agentspace-kernel_copilot-config",
        )

    async def create_session(
        self,
        *,
        session_id: str,
        harness: str,
        env: dict[str, str],
        cwd: str | None,
        additional_paths: tuple[str, ...],
    ) -> tuple[str, str]:
        container_name = f"agentspace-kernel-{session_id[:12]}"
        base_url = self._base_url_template.format(container_name=container_name)
        await asyncio.to_thread(
            self._run_container,
            container_name,
            harness,
            env,
            cwd,
            additional_paths,
        )
        await self._wait_until_ready(base_url)
        return container_name, base_url

    async def send_message(self, *, base_url: str, message: str) -> list[KernelEvent]:
        payload = {"message": message}
        async with httpx.AsyncClient(timeout=self._startup_timeout) as client:
            response = await client.post(f"{base_url}/messages", json=payload)
        response.raise_for_status()
        raw_events = response.json()["events"]
        return [KernelEvent(**event) for event in raw_events]

    async def summary(self, *, base_url: str) -> dict[str, Any]:
        async with httpx.AsyncClient(timeout=self._startup_timeout) as client:
            response = await client.get(f"{base_url}/session")
        response.raise_for_status()
        return dict(response.json())

    async def history(self, *, base_url: str) -> list[list[KernelEvent]]:
        async with httpx.AsyncClient(timeout=self._startup_timeout) as client:
            response = await client.get(f"{base_url}/history")
        response.raise_for_status()
        raw_history = response.json()["history"]
        return [[KernelEvent(**event) for event in turn] for turn in raw_history]

    async def destroy_session(self, *, container_name: str) -> None:
        await asyncio.to_thread(self._remove_container, container_name)

    def _run_container(
        self,
        container_name: str,
        harness: str,
        env: dict[str, str],
        cwd: str | None,
        additional_paths: tuple[str, ...],
    ) -> None:
        environment = dict(env)
        environment["KERNEL_HARNESS"] = harness
        if cwd is not None:
            environment["KERNEL_WORKDIR"] = cwd
        if additional_paths:
            environment["KERNEL_ADDITIONAL_PATHS"] = os.pathsep.join(additional_paths)

        self._client.containers.run(
            self._kernel_image,
            auto_remove=True,
            detach=True,
            entrypoint=[
                "/usr/local/bin/uv",
                "run",
                "--no-dev",
                "--package",
                "kernel-host",
                "-m",
                "kernel_host.api_main",
            ],
            environment=environment,
            name=container_name,
            network=self._kernel_network,
            volumes={
                self._copilot_volume: {
                    "bind": "/root/.copilot",
                    "mode": "rw",
                },
            },
        )

    def _remove_container(self, container_name: str) -> None:
        try:
            container = self._client.containers.get(container_name)
        except NotFound:
            return
        container.remove(force=True)

    async def _wait_until_ready(self, base_url: str) -> None:
        async with httpx.AsyncClient(timeout=5.0) as client:
            deadline = asyncio.get_running_loop().time() + self._startup_timeout
            while True:
                try:
                    response = await client.get(f"{base_url}/healthz")
                    if response.status_code == 200:
                        return
                except httpx.HTTPError:
                    pass
                if asyncio.get_running_loop().time() >= deadline:
                    msg = f"kernel container at {base_url} did not become ready"
                    raise TimeoutError(msg)
                await asyncio.sleep(1)


class AgentHost:
    def __init__(self, runtime: KernelRuntime | None = None) -> None:
        self._runtime = runtime or DockerKernelRuntime()
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
        merged_env = dict(os.environ)
        merged_env.update(env or {})
        container_name, base_url = await self._runtime.create_session(
            session_id=session_id,
            harness=harness,
            env=merged_env,
            cwd=cwd,
            additional_paths=additional_paths,
        )
        record = SessionRecord(
            session_id=session_id,
            harness=harness,
            container_name=container_name,
            base_url=base_url,
            env=merged_env,
            cwd=cwd,
            additional_paths=additional_paths,
        )
        session_summary = await self._runtime.summary(base_url=base_url)
        record.resume_token = _as_resume_token(session_summary.get("resume_token"))
        record.status = _as_status(session_summary.get("status"), KernelStatus.IDLE)
        async with self._lock:
            self._sessions[session_id] = record
        return record.summary()

    async def send_message(self, session_id: str, message: str) -> list[KernelEvent]:
        record = self._get_session(session_id)
        events = await self._runtime.send_message(
            base_url=record.base_url,
            message=message,
        )
        record.history.append(events)
        record.status = _derive_status(events, record.status)
        session_summary = await self._runtime.summary(base_url=record.base_url)
        record.resume_token = _as_resume_token(session_summary.get("resume_token"))
        record.status = _as_status(session_summary.get("status"), record.status)
        return events

    async def destroy_session(self, session_id: str) -> None:
        async with self._lock:
            record = self._sessions.pop(session_id, None)
        if record is None:
            raise SessionNotFoundError(session_id)
        await self._runtime.destroy_session(container_name=record.container_name)

    async def reset_session(self, session_id: str) -> dict[str, Any]:
        record = self._get_session(session_id)
        harness = record.harness
        env = dict(record.env)
        cwd = record.cwd
        additional_paths = record.additional_paths
        await self.destroy_session(session_id)
        return await self.create_session(
            harness=harness,
            env=env,
            cwd=cwd,
            additional_paths=additional_paths,
        )

    async def get_session(self, session_id: str) -> dict[str, Any]:
        record = self._get_session(session_id)
        session_summary = await self._runtime.summary(base_url=record.base_url)
        record.resume_token = _as_resume_token(session_summary.get("resume_token"))
        record.status = _as_status(session_summary.get("status"), record.status)
        return record.summary()

    async def list_sessions(self) -> list[dict[str, Any]]:
        async with self._lock:
            session_ids = list(self._sessions)
        return [await self.get_session(session_id) for session_id in session_ids]

    async def history(self, session_id: str) -> list[list[KernelEvent]]:
        record = self._get_session(session_id)
        history = await self._runtime.history(base_url=record.base_url)
        record.history = history
        return list(history)

    def _get_session(self, session_id: str) -> SessionRecord:
        try:
            return self._sessions[session_id]
        except KeyError as exc:
            raise SessionNotFoundError(session_id) from exc


def _derive_status(
    events: list[KernelEvent],
    fallback_status: KernelStatus,
) -> KernelStatus:
    for event in reversed(events):
        if event.type == EventType.STATUS and event.status is not None:
            return event.status
    return fallback_status


def _as_status(value: object, fallback_status: KernelStatus) -> KernelStatus:
    if isinstance(value, str):
        return KernelStatus(value)
    return fallback_status


def _as_resume_token(value: object) -> str | None:
    if isinstance(value, str) and value:
        return value
    return None
