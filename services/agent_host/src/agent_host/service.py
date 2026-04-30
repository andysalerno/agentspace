from __future__ import annotations

import asyncio
import json
import logging
import os
import uuid
from dataclasses import dataclass, field
from time import perf_counter
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
    HarnessName.ACP: "/workspace/.agents/skills",
    HarnessName.CLAUDE_CODE: "/skills",
    HarnessName.COPILOT_CLI: "/root/.copilot/skills",
    HarnessName.CODEX: "/skills",
    HarnessName.ECHO: "/skills",
    HarnessName.OPENCODE: "/root/.config/opencode/skills",
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

    async def container_logs(
        self,
        *,
        session: KernelRuntimeSession,
        tail: int | None,
    ) -> list[str]: ...

    async def stats(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> dict[str, Any] | None: ...

    def container_name(self, *, session: KernelRuntimeSession) -> str | None: ...

    def vscode_url(self, *, session: KernelRuntimeSession) -> str | None: ...

    def free_port_url(self, *, session: KernelRuntimeSession) -> str | None: ...

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
    container_name: str | None = None
    vscode_url: str | None = None
    free_port_url: str | None = None
    stats: dict[str, Any] | None = None

    def summary(self) -> dict[str, Any]:
        return {
            "session_id": self.session_id,
            "harness": self.harness.value,
            "status": self.status,
            "turns": len(self.history),
            "resume_token": self.resume_token,
            "additional_paths": list(self.additional_paths),
            "container_name": self.container_name,
            "vscode_url": self.vscode_url,
            "free_port_url": self.free_port_url,
            "stats": self.stats,
        }


class DockerKernelRuntime:
    def __init__(self) -> None:
        self._client_instance: docker.DockerClient | None = None
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
        self._vscode_container_port = int(
            os.environ.get("AGENT_HOST_KERNEL_VSCODE_CONTAINER_PORT", "8080"),
        )
        self._vscode_host_ip = os.environ.get(
            "AGENT_HOST_KERNEL_VSCODE_HOST_IP",
            "0.0.0.0",  # noqa: S104 - code-server must be reachable off-host.
        )
        self._vscode_url_template = os.environ.get(
            "AGENT_HOST_KERNEL_VSCODE_URL_TEMPLATE",
            "http://127.0.0.1:{host_port}",
        )
        self._free_port_container_port = int(
            os.environ.get("AGENT_HOST_KERNEL_FREE_PORT_CONTAINER_PORT", "8081"),
        )
        self._free_port_host_ip = os.environ.get(
            "AGENT_HOST_KERNEL_FREE_PORT_HOST_IP",
            self._vscode_host_ip,
        )
        self._free_port_url_template = os.environ.get(
            "AGENT_HOST_KERNEL_FREE_PORT_URL_TEMPLATE",
            "http://127.0.0.1:{host_port}",
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

    @property
    def _client(self) -> docker.DockerClient:
        # Lazy: avoid connecting to the Docker daemon until first use,
        # so importing this module (and constructing AgentHost for tests
        # or app startup) does not require a running daemon.
        if self._client_instance is None:
            self._client_instance = docker.from_env()
        return self._client_instance

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
        logger.info(
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
        vscode_url = await asyncio.to_thread(
            self._vscode_url_for_container,
            container_name,
        )
        free_port_url = await asyncio.to_thread(
            self._free_port_url_for_container,
            container_name,
        )
        await self._wait_until_ready(base_url)
        return KernelRuntimeSession(
            value=DockerKernelSession(
                container_name=container_name,
                base_url=base_url,
                vscode_url=vscode_url,
                free_port_url=free_port_url,
            ),
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
            started_at = perf_counter()
            first_event_seen = False
            timeout = httpx.Timeout(self._startup_timeout, read=None)
            logger.info(
                "agent_host docker stream start: container=%s message_chars=%d",
                handle.container_name,
                len(message),
            )
            async with (
                httpx.AsyncClient(timeout=timeout) as client,
                client.stream(
                    "POST",
                    f"{handle.base_url}/messages/stream",
                    json=payload,
                ) as response,
            ):
                logger.info(
                    "docker response: container=%s elapsed_ms=%.1f status=%d",
                    handle.container_name,
                    (perf_counter() - started_at) * 1000,
                    response.status_code,
                )
                response.raise_for_status()
                async for line in response.aiter_lines():
                    if not line:
                        continue
                    raw_event = json.loads(line)
                    if not isinstance(raw_event, dict):
                        continue
                    if not first_event_seen:
                        first_event_seen = True
                        logger.info(
                            "docker first event: container=%s elapsed_ms=%.1f",
                            handle.container_name,
                            (perf_counter() - started_at) * 1000,
                        )
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

    async def container_logs(
        self,
        *,
        session: KernelRuntimeSession,
        tail: int | None,
    ) -> list[str]:
        handle = self._docker_session(session)
        return await asyncio.to_thread(
            self._docker_container_logs,
            handle.container_name,
            tail,
        )

    async def stats(
        self,
        *,
        session: KernelRuntimeSession,
    ) -> dict[str, Any] | None:
        handle = self._docker_session(session)
        return await asyncio.to_thread(
            self._docker_container_stats,
            handle.container_name,
        )

    def container_name(self, *, session: KernelRuntimeSession) -> str | None:
        return self._docker_session(session).container_name

    def vscode_url(self, *, session: KernelRuntimeSession) -> str | None:
        return self._docker_session(session).vscode_url

    def free_port_url(self, *, session: KernelRuntimeSession) -> str | None:
        return self._docker_session(session).free_port_url

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
        environment["KERNEL_VSCODE_ENABLED"] = environment.get(
            "KERNEL_VSCODE_ENABLED",
            "1",
        )
        environment["KERNEL_FREE_PORT"] = str(self._free_port_container_port)
        vscode_enabled = environment["KERNEL_VSCODE_ENABLED"].lower() not in {
            "0",
            "false",
            "no",
            "off",
        }
        ports: dict[str, tuple[str, int]] = {
            f"{self._free_port_container_port}/tcp": (self._free_port_host_ip, 0),
        }
        if vscode_enabled:
            ports[f"{self._vscode_container_port}/tcp"] = (self._vscode_host_ip, 0)

        logger.info(
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
            ports=ports,
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

    def _vscode_url_for_container(self, container_name: str) -> str | None:
        host_port = self._container_host_port(
            container_name,
            self._vscode_container_port,
        )
        if host_port is None:
            return None
        return self._vscode_url_template.format(
            container_name=container_name,
            host_ip=self._vscode_host_ip,
            host_port=host_port,
            container_port=self._vscode_container_port,
        )

    def _free_port_url_for_container(self, container_name: str) -> str | None:
        host_port = self._container_host_port(
            container_name,
            self._free_port_container_port,
        )
        if host_port is None:
            return None
        return self._free_port_url_template.format(
            container_name=container_name,
            host_ip=self._free_port_host_ip,
            host_port=host_port,
            container_port=self._free_port_container_port,
        )

    def _container_host_port(
        self,
        container_name: str,
        container_port: int,
    ) -> str | None:
        try:
            container = self._client.containers.get(container_name)
            container.reload()
        except (DockerException, NotFound) as exc:
            logger.warning("failed to inspect container %s: %s", container_name, exc)
            return None
        attrs = container.attrs
        network_settings = _as_dict(attrs.get("NetworkSettings"))
        ports = _as_dict(network_settings.get("Ports"))
        raw_bindings = ports.get(f"{container_port}/tcp")
        if not isinstance(raw_bindings, list) or not raw_bindings:
            return None
        bindings = cast("list[object]", raw_bindings)
        first = bindings[0]
        if not isinstance(first, dict):
            return None
        binding = cast("dict[str, object]", first)
        host_port = binding.get("HostPort")
        return host_port if isinstance(host_port, str) and host_port else None

    def _remove_container(self, container_name: str) -> None:
        try:
            container = self._client.containers.get(container_name)
        except NotFound:
            return
        container.remove(force=True)

    def _docker_container_logs(
        self,
        container_name: str,
        tail: int | None,
    ) -> list[str]:
        try:
            container = self._client.containers.get(container_name)
        except NotFound:
            return []
        kwargs: dict[str, Any] = {"stdout": True, "stderr": True}
        if tail is not None:
            kwargs["tail"] = tail
        try:
            raw = cast(
                "object",
                container.logs(**kwargs),  # pyright: ignore[reportUnknownMemberType]
            )
        except DockerException as exc:
            logger.warning(
                "failed to fetch container logs for %s: %s",
                container_name,
                exc,
            )
            return []
        if isinstance(raw, bytes):
            return raw.decode(errors="replace").splitlines()
        # Fallback: docker-py can return a generator if stream=True; we never
        # ask for it, but be defensive.
        return []

    def _docker_container_stats(
        self,
        container_name: str,
    ) -> dict[str, Any] | None:
        try:
            container = self._client.containers.get(container_name)
        except NotFound:
            return None
        try:
            raw = cast(
                "object",
                container.stats(stream=False),  # pyright: ignore[reportUnknownMemberType]
            )
        except DockerException as exc:
            logger.warning(
                "failed to fetch container stats for %s: %s",
                container_name,
                exc,
            )
            return None
        if not isinstance(raw, dict):
            return None
        return _summarize_docker_stats(cast("dict[str, Any]", raw))

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
        harness: HarnessName = HarnessName.ACP,
        env: dict[str, str] | None = None,
        additional_paths: tuple[str, ...] = (),
        skills: tuple[str, ...] = (),
    ) -> dict[str, Any]:
        session_id = uuid.uuid4().hex
        caller_env = env or {}
        merged_env = dict(os.environ)
        merged_env.update(caller_env)
        logger.info(
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
            container_name=self._runtime.container_name(session=runtime_session),
            vscode_url=self._runtime.vscode_url(session=runtime_session),
            free_port_url=self._runtime.free_port_url(session=runtime_session),
        )
        session_summary = await self._runtime.summary(session=runtime_session)
        record.resume_token = _as_resume_token(session_summary.get("resume_token"))
        record.status = _as_status(session_summary.get("status"), KernelStatus.IDLE)
        record.vscode_url = _as_optional_str(
            session_summary.get("vscode_url"),
            record.vscode_url,
        )
        record.free_port_url = _as_optional_str(
            session_summary.get("free_port_url"),
            record.free_port_url,
        )
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
        started_at = perf_counter()
        logger.info(
            "agent_host stream start: session=%s harness=%s message_chars=%d",
            session_id,
            record.harness.value,
            len(message),
        )
        stream = self._runtime.stream_message(
            session=record.runtime_session,
            message=message,
        )

        async def iterator() -> AsyncIterator[KernelEvent]:
            try:
                async for event in stream:
                    if not events:
                        logger.info(
                            "agent_host first: session=%s elapsed_ms=%.1f type=%s",
                            session_id,
                            (perf_counter() - started_at) * 1000,
                            event.type,
                        )
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
                record.vscode_url = _as_optional_str(
                    session_summary.get("vscode_url"),
                    record.vscode_url,
                )
                record.free_port_url = _as_optional_str(
                    session_summary.get("free_port_url"),
                    record.free_port_url,
                )
                logger.info(
                    "agent_host final: session=%s elapsed_ms=%.1f events=%d status=%s",
                    session_id,
                    (perf_counter() - started_at) * 1000,
                    len(events),
                    record.status,
                )

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

    async def get_session(
        self,
        session_id: str,
        *,
        with_stats: bool = False,
    ) -> dict[str, Any]:
        record = self._get_session(session_id)
        if with_stats:
            session_summary, stats = await asyncio.gather(
                self._runtime.summary(session=record.runtime_session),
                self._runtime.stats(session=record.runtime_session),
            )
            # ``record.stats`` is a cache of the last fetched stats payload;
            # it is only refreshed when the caller asks for ``with_stats``.
            record.stats = stats
        else:
            session_summary = await self._runtime.summary(
                session=record.runtime_session,
            )
        record.resume_token = _as_resume_token(session_summary.get("resume_token"))
        record.status = _as_status(session_summary.get("status"), record.status)
        record.vscode_url = _as_optional_str(
            session_summary.get("vscode_url"),
            record.vscode_url,
        )
        record.free_port_url = _as_optional_str(
            session_summary.get("free_port_url"),
            record.free_port_url,
        )
        return record.summary()

    async def list_sessions(
        self,
        *,
        with_stats: bool = False,
    ) -> list[dict[str, Any]]:
        async with self._lock:
            session_ids = list(self._sessions)
        results = await asyncio.gather(
            *(
                self.get_session(session_id, with_stats=with_stats)
                for session_id in session_ids
            ),
            return_exceptions=True,
        )
        summaries: list[dict[str, Any]] = []
        for session_id, result in zip(session_ids, results, strict=True):
            if isinstance(result, BaseException):
                logger.warning(
                    "failed to fetch summary for session %s: %s",
                    session_id,
                    result,
                )
                # Keep the row visible so the operator can still see and kill
                # a misbehaving kernel; fall back to the last cached summary.
                record = self._sessions.get(session_id)
                if record is not None:
                    summaries.append(record.summary())
                continue
            summaries.append(result)
        return summaries

    async def history(self, session_id: str) -> list[list[KernelEvent]]:
        record = self._get_session(session_id)
        history = await self._runtime.history(session=record.runtime_session)
        record.history = history
        return list(history)

    async def logs(self, session_id: str) -> list[str]:
        record = self._get_session(session_id)
        return await self._runtime.logs(session=record.runtime_session)

    async def container_logs(
        self,
        session_id: str,
        *,
        tail: int | None = 2000,
    ) -> list[str]:
        record = self._get_session(session_id)
        return await self._runtime.container_logs(
            session=record.runtime_session,
            tail=tail,
        )

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
        if event.type == EventType.SESSION_STATUS and event.status is not None:
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


def _as_optional_str(value: object, fallback: str | None = None) -> str | None:
    if isinstance(value, str) and value:
        return value
    return fallback


@dataclass(frozen=True, slots=True)
class DockerKernelSession:
    container_name: str
    base_url: str
    vscode_url: str | None = None
    free_port_url: str | None = None


def _summarize_docker_stats(raw: dict[str, Any]) -> dict[str, Any] | None:
    """Convert a raw ``docker stats`` payload into a small summary dict.

    Returns a dict with ``cpu_percent``, ``memory_usage_bytes``,
    ``memory_limit_bytes``, ``memory_percent``. Any missing field is ``None``.

    Returns ``None`` if the payload is unusable (e.g. container not running).
    """
    cpu_stats = _as_dict(raw.get("cpu_stats"))
    precpu_stats = _as_dict(raw.get("precpu_stats"))
    memory_stats = _as_dict(raw.get("memory_stats"))

    cpu_percent = _compute_cpu_percent(cpu_stats, precpu_stats)
    memory_usage = _compute_memory_usage(memory_stats)
    memory_limit = _as_int(memory_stats.get("limit"))
    memory_percent: float | None
    if memory_usage is not None and memory_limit is not None and memory_limit > 0:
        memory_percent = (memory_usage / memory_limit) * 100.0
    else:
        memory_percent = None

    if cpu_percent is None and memory_usage is None and memory_limit is None:
        return None

    return {
        "cpu_percent": cpu_percent,
        "memory_usage_bytes": memory_usage,
        "memory_limit_bytes": memory_limit,
        "memory_percent": memory_percent,
    }


def _compute_cpu_percent(
    cpu_stats: dict[str, Any],
    precpu_stats: dict[str, Any],
) -> float | None:
    cpu_usage = _as_dict(cpu_stats.get("cpu_usage"))
    precpu_usage = _as_dict(precpu_stats.get("cpu_usage"))
    total = _as_int(cpu_usage.get("total_usage"))
    pre_total = _as_int(precpu_usage.get("total_usage"))
    system = _as_int(cpu_stats.get("system_cpu_usage"))
    pre_system = _as_int(precpu_stats.get("system_cpu_usage"))

    if total is None or pre_total is None or system is None or pre_system is None:
        return None

    cpu_delta = total - pre_total
    system_delta = system - pre_system
    if system_delta <= 0 or cpu_delta < 0:
        return None

    online_cpus = _as_int(cpu_stats.get("online_cpus"))
    if online_cpus is None or online_cpus <= 0:
        per_cpu: object = cpu_usage.get("percpu_usage")
        online_cpus = (
            len(cast("list[object]", per_cpu)) if isinstance(per_cpu, list) else 1
        )
    if online_cpus <= 0:
        online_cpus = 1

    return (cpu_delta / system_delta) * online_cpus * 100.0


def _compute_memory_usage(memory_stats: dict[str, Any]) -> int | None:
    usage = _as_int(memory_stats.get("usage"))
    if usage is None:
        return None
    stats_section = _as_dict(memory_stats.get("stats"))
    # cgroup v1 reports ``cache``; cgroup v2 reports ``inactive_file``.
    cache = _as_int(stats_section.get("cache"))
    if cache is None:
        cache = _as_int(stats_section.get("inactive_file"))
    if cache is not None and cache <= usage:
        return usage - cache
    return usage


def _as_dict(value: object) -> dict[str, Any]:
    if isinstance(value, dict):
        return cast("dict[str, Any]", value)
    return {}


def _as_int(value: object) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    return None
