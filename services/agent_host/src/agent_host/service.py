from __future__ import annotations

import asyncio
import json
import logging
import os
import uuid
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Protocol, cast

import docker
import httpx
from docker.errors import DockerException, NotFound
from kernel.events import EventType, KernelEvent, KernelStatus
from kernel_host.registry import HarnessName

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Awaitable, Callable

    type AcloseFn = Callable[[], Awaitable[object]]

logger = logging.getLogger(__name__)

# Where each harness expects to find skill directories inside the container.
SKILLS_MOUNT_PATHS: dict[HarnessName, str] = {
    HarnessName.CLAUDE_CODE: "/skills",
    HarnessName.COPILOT_CLI: "/root/.copilot/skills",
    HarnessName.CODEX: "/skills",
    HarnessName.ECHO: "/skills",
}


class SessionNotFoundError(KeyError):
    pass


@dataclass(frozen=True, slots=True)
class KernelRuntimeSession:
    value: object


class KernelRuntime(Protocol):
    async def create_session(
        self,
        *,
        session_id: str,
        harness: HarnessName,
        env: dict[str, str],
        additional_paths: tuple[str, ...],
        skills: tuple[str, ...] = (),
    ) -> KernelRuntimeSession: ...

    async def send_message(
        self,
        *,
        session: KernelRuntimeSession,
        message: str,
    ) -> list[KernelEvent]: ...

    def stream_message(
        self,
        *,
        session: KernelRuntimeSession,
        message: str,
    ) -> AsyncIterator[KernelEvent]: ...

    async def summary(self, *, session: KernelRuntimeSession) -> dict[str, Any]: ...

    async def history(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> list[list[KernelEvent]]: ...

    async def logs(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> list[str]: ...

    async def destroy_session(self, *, session: KernelRuntimeSession) -> None: ...


def _empty_history() -> list[list[KernelEvent]]:
    return []


@dataclass(slots=True)
class SessionRecord:
    session_id: str
    harness: HarnessName
    runtime_session: KernelRuntimeSession
    env: dict[str, str]
    additional_paths: tuple[str, ...]
    skills: tuple[str, ...] = ()
    history: list[list[KernelEvent]] = field(default_factory=_empty_history)
    status: KernelStatus = KernelStatus.IDLE
    resume_token: str | None = None

    def summary(self) -> dict[str, Any]:
        return {
            "session_id": self.session_id,
            "harness": self.harness.value,
            "status": self.status,
            "turns": len(self.history),
            "resume_token": self.resume_token,
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
        self._skills_volume = os.environ.get(
            "AGENT_HOST_SKILLS_VOLUME",
            "agentspace-skills",
        )
        self._skills_dir = os.environ.get(
            "AGENT_HOST_SKILLS_DIR",
            "/skills",
        )

    async def create_session(
        self,
        *,
        session_id: str,
        harness: HarnessName,
        env: dict[str, str],
        additional_paths: tuple[str, ...],
        skills: tuple[str, ...] = (),
    ) -> KernelRuntimeSession:
        container_name = f"agentspace-kernel-{session_id[:12]}"
        base_url = self._base_url_template.format(container_name=container_name)
        logger.debug(
            "creating kernel container: name=%s harness=%s"
            " env_keys=%s additional_paths=%s skills=%s",
            container_name,
            harness.value,
            sorted(env.keys()),
            additional_paths,
            skills,
        )
        await asyncio.to_thread(
            self._run_container,
            container_name,
            harness,
            env,
            additional_paths,
            skills,
        )
        await self._wait_until_ready(base_url)
        return KernelRuntimeSession(
            value=DockerKernelSession(container_name=container_name, base_url=base_url),
        )

    async def send_message(
        self,
        *,
        session: KernelRuntimeSession,
        message: str,
    ) -> list[KernelEvent]:
        return [
            event
            async for event in self.stream_message(
                session=session,
                message=message,
            )
        ]

    def stream_message(
        self,
        *,
        session: KernelRuntimeSession,
        message: str,
    ) -> AsyncIterator[KernelEvent]:
        handle = self._docker_session(session)
        payload = {"message": message}

        async def iterator() -> AsyncIterator[KernelEvent]:
            timeout = httpx.Timeout(self._startup_timeout, read=None)
            async with (
                httpx.AsyncClient(timeout=timeout) as client,
                client.stream(
                    "POST",
                    f"{handle.base_url}/messages/stream",
                    json=payload,
                ) as response,
            ):
                response.raise_for_status()
                async for line in response.aiter_lines():
                    if not line:
                        continue
                    raw_event = json.loads(line)
                    if not isinstance(raw_event, dict):
                        continue
                    yield KernelEvent(**cast("dict[str, Any]", raw_event))

        return iterator()

    async def summary(self, *, session: KernelRuntimeSession) -> dict[str, Any]:
        handle = self._docker_session(session)
        async with httpx.AsyncClient(timeout=self._startup_timeout) as client:
            response = await client.get(f"{handle.base_url}/session")
        response.raise_for_status()
        return dict(response.json())

    async def history(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> list[list[KernelEvent]]:
        handle = self._docker_session(session)
        async with httpx.AsyncClient(timeout=self._startup_timeout) as client:
            response = await client.get(f"{handle.base_url}/history")
        response.raise_for_status()
        raw_history = response.json()["history"]
        return [[KernelEvent(**event) for event in turn] for turn in raw_history]

    async def logs(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> list[str]:
        handle = self._docker_session(session)
        async with httpx.AsyncClient(timeout=self._startup_timeout) as client:
            response = await client.get(f"{handle.base_url}/logs")
        response.raise_for_status()
        return list(response.json()["lines"])

    async def destroy_session(self, *, session: KernelRuntimeSession) -> None:
        handle = self._docker_session(session)
        await asyncio.to_thread(self._remove_container, handle.container_name)

    def _run_container(
        self,
        container_name: str,
        harness: HarnessName,
        env: dict[str, str],
        additional_paths: tuple[str, ...],
        skills: tuple[str, ...] = (),
    ) -> None:
        environment = dict(env)
        environment["KERNEL_HARNESS"] = harness.value
        if additional_paths:
            environment["KERNEL_ADDITIONAL_PATHS"] = os.pathsep.join(additional_paths)

        skills_mount = SKILLS_MOUNT_PATHS.get(harness, "/skills")
        skills_staging = "/mnt/all-skills"
        environment["KERNEL_SKILLS_DIR"] = skills_mount
        environment["KERNEL_SKILLS_STAGING_DIR"] = skills_staging
        environment["KERNEL_ENABLED_SKILLS"] = ",".join(skills)

        logger.debug(
            "container %s final env: %s",
            container_name,
            environment,
        )

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
            labels={"agentspace.role": "kernel"},
            name=container_name,
            network=self._kernel_network,
            volumes={
                self._copilot_volume: {
                    "bind": "/root/.copilot",
                    "mode": "rw",
                },
                self._skills_volume: {
                    "bind": skills_staging,
                    "mode": "ro",
                },
            },
        )

    def _docker_session(self, session: KernelRuntimeSession) -> DockerKernelSession:
        if not isinstance(session.value, DockerKernelSession):
            msg = f"unsupported runtime session handle: {type(session.value)!r}"
            raise TypeError(msg)
        return session.value

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
        harness: HarnessName = HarnessName.COPILOT_CLI,
        env: dict[str, str] | None = None,
        additional_paths: tuple[str, ...] = (),
        skills: tuple[str, ...] = (),
    ) -> dict[str, Any]:
        session_id = uuid.uuid4().hex
        caller_env = env or {}
        merged_env = dict(os.environ)
        merged_env.update(caller_env)
        logger.debug(
            "creating session %s: harness=%s caller_env_keys=%s skills=%s",
            session_id,
            harness.value,
            sorted(caller_env.keys()),
            skills,
        )
        runtime_session = await self._runtime.create_session(
            session_id=session_id,
            harness=harness,
            env=merged_env,
            additional_paths=additional_paths,
            skills=skills,
        )
        record = SessionRecord(
            session_id=session_id,
            harness=harness,
            runtime_session=runtime_session,
            env=merged_env,
            additional_paths=additional_paths,
            skills=skills,
        )
        session_summary = await self._runtime.summary(session=runtime_session)
        record.resume_token = _as_resume_token(session_summary.get("resume_token"))
        record.status = _as_status(session_summary.get("status"), KernelStatus.IDLE)
        async with self._lock:
            self._sessions[session_id] = record
        return record.summary()

    async def send_message(self, session_id: str, message: str) -> list[KernelEvent]:
        return [event async for event in self.stream_message(session_id, message)]

    def stream_message(
        self,
        session_id: str,
        message: str,
    ) -> AsyncIterator[KernelEvent]:
        record = self._get_session(session_id)
        events: list[KernelEvent] = []
        stream = self._runtime.stream_message(
            session=record.runtime_session,
            message=message,
        )

        async def iterator() -> AsyncIterator[KernelEvent]:
            try:
                async for event in stream:
                    events.append(event)
                    yield event
            finally:
                aclose = getattr(stream, "aclose", None)
                if callable(aclose):
                    await cast("AcloseFn", aclose)()
                if events:
                    record.history.append(list(events))
                    record.status = _derive_status(events, record.status)
                session_summary = await self._runtime.summary(
                    session=record.runtime_session,
                )
                record.resume_token = _as_resume_token(
                    session_summary.get("resume_token"),
                )
                record.status = _as_status(session_summary.get("status"), record.status)

        return iterator()

    async def destroy_session(self, session_id: str) -> None:
        async with self._lock:
            record = self._sessions.pop(session_id, None)
        if record is None:
            raise SessionNotFoundError(session_id)
        await self._runtime.destroy_session(session=record.runtime_session)

    async def destroy_all_sessions(self) -> None:
        """Destroy all active sessions. Called during shutdown."""
        async with self._lock:
            records = list(self._sessions.values())
            self._sessions.clear()
        for record in records:
            try:
                await self._runtime.destroy_session(session=record.runtime_session)
            except (OSError, DockerException, httpx.HTTPError):
                logger.warning(
                    "failed to destroy kernel for session %s",
                    record.session_id,
                    exc_info=True,
                )

    async def reset_session(self, session_id: str) -> dict[str, Any]:
        record = self._get_session(session_id)
        harness = record.harness
        env = dict(record.env)
        additional_paths = record.additional_paths
        skills = record.skills
        await self.destroy_session(session_id)
        return await self.create_session(
            harness=harness,
            env=env,
            additional_paths=additional_paths,
            skills=skills,
        )

    async def get_session(self, session_id: str) -> dict[str, Any]:
        record = self._get_session(session_id)
        session_summary = await self._runtime.summary(session=record.runtime_session)
        record.resume_token = _as_resume_token(session_summary.get("resume_token"))
        record.status = _as_status(session_summary.get("status"), record.status)
        return record.summary()

    async def list_sessions(self) -> list[dict[str, Any]]:
        async with self._lock:
            session_ids = list(self._sessions)
        return [await self.get_session(session_id) for session_id in session_ids]

    async def history(self, session_id: str) -> list[list[KernelEvent]]:
        record = self._get_session(session_id)
        history = await self._runtime.history(session=record.runtime_session)
        record.history = history
        return list(history)

    async def logs(self, session_id: str) -> list[str]:
        record = self._get_session(session_id)
        return await self._runtime.logs(session=record.runtime_session)

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


@dataclass(frozen=True, slots=True)
class DockerKernelSession:
    container_name: str
    base_url: str
