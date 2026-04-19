"""Gateway lifecycle management for agent_host.

Mirrors the kernel runtime pattern: gateways run as Docker containers on the
shared agentspace network.  ``DockerGatewayRuntime`` spawns and tears down
containers; ``GatewayHost`` is the in-memory bookkeeper that tracks them
for the agent_host service.
"""

from __future__ import annotations

import asyncio
import logging
import os
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Protocol

import docker
import httpx
from docker.errors import DockerException, NotFound

if TYPE_CHECKING:
    from collections.abc import Iterable

logger = logging.getLogger(__name__)


class GatewayNotFoundError(KeyError):
    pass


class GatewayAlreadyExistsError(ValueError):
    pass


@dataclass(frozen=True, slots=True)
class GatewayRuntimeSession:
    """Opaque handle returned by a ``GatewayRuntime`` to identify a gateway."""

    value: object


@dataclass(frozen=True, slots=True)
class DockerGatewaySession:
    container_name: str
    base_url: str


class GatewayRuntime(Protocol):
    async def create_gateway(
        self,
        *,
        gateway_id: str,
        gateway_type: str,
        agent_id: str,
        env: dict[str, str],
    ) -> GatewayRuntimeSession: ...

    async def destroy_gateway(self, *, session: GatewayRuntimeSession) -> None: ...

    async def status(self, *, session: GatewayRuntimeSession) -> dict[str, object]: ...

    async def logs(self, *, session: GatewayRuntimeSession) -> list[str]: ...


def _empty_env() -> dict[str, str]:
    return {}


@dataclass(slots=True)
class GatewayRecord:
    gateway_id: str
    gateway_type: str
    agent_id: str
    runtime_session: GatewayRuntimeSession
    env: dict[str, str] = field(default_factory=_empty_env)

    def summary(self) -> dict[str, object]:
        return {
            "gateway_id": self.gateway_id,
            "type": self.gateway_type,
            "agent_id": self.agent_id,
        }


class DockerGatewayRuntime:
    def __init__(self) -> None:
        self._client_instance: docker.DockerClient | None = None
        self._gateway_image = os.environ.get(
            "AGENT_HOST_GATEWAY_IMAGE",
            "agentspace-gateway-gateway:latest",
        )
        self._network = os.environ.get(
            "AGENT_HOST_DOCKER_NETWORK",
            "agentspace-stack",
        )
        self._base_url_template = os.environ.get(
            "AGENT_HOST_GATEWAY_BASE_URL_TEMPLATE",
            "http://{container_name}:8000",
        )
        self._client_service_url = os.environ.get(
            "AGENT_HOST_GATEWAY_CLIENT_SERVICE_URL",
            "http://client-service:8002",
        )
        self._startup_timeout = float(
            os.environ.get("AGENT_HOST_GATEWAY_STARTUP_TIMEOUT", "60"),
        )

    @property
    def _client(self) -> docker.DockerClient:
        if self._client_instance is None:
            self._client_instance = docker.from_env()
        return self._client_instance

    async def create_gateway(
        self,
        *,
        gateway_id: str,
        gateway_type: str,
        agent_id: str,
        env: dict[str, str],
    ) -> GatewayRuntimeSession:
        container_name = f"agentspace-gateway-{gateway_id}"
        base_url = self._base_url_template.format(container_name=container_name)
        logger.info(
            "creating gateway container: name=%s type=%s agent=%s env_keys=%s",
            container_name,
            gateway_type,
            agent_id,
            sorted(env.keys()),
        )
        await asyncio.to_thread(
            self._run_container,
            container_name,
            gateway_id,
            gateway_type,
            agent_id,
            env,
        )
        await self._wait_until_ready(base_url)
        return GatewayRuntimeSession(
            value=DockerGatewaySession(
                container_name=container_name,
                base_url=base_url,
            ),
        )

    async def destroy_gateway(self, *, session: GatewayRuntimeSession) -> None:
        handle = self._docker_session(session)
        await asyncio.to_thread(self._remove_container, handle.container_name)

    async def status(self, *, session: GatewayRuntimeSession) -> dict[str, object]:
        handle = self._docker_session(session)
        try:
            async with httpx.AsyncClient(timeout=self._startup_timeout) as client:
                response = await client.get(f"{handle.base_url}/status")
            response.raise_for_status()
        except httpx.HTTPError as exc:
            return {
                "status": "error",
                "last_error": f"failed to reach gateway: {exc}",
            }
        return dict(response.json())

    async def logs(self, *, session: GatewayRuntimeSession) -> list[str]:
        handle = self._docker_session(session)
        # Prefer the container's own /logs endpoint; fall back to docker logs.
        try:
            async with httpx.AsyncClient(timeout=self._startup_timeout) as client:
                response = await client.get(f"{handle.base_url}/logs")
            response.raise_for_status()
            return list(response.json().get("lines", []))
        except httpx.HTTPError:
            return await asyncio.to_thread(
                self._docker_container_logs,
                handle.container_name,
            )

    def _run_container(
        self,
        container_name: str,
        gateway_id: str,
        gateway_type: str,
        agent_id: str,
        env: dict[str, str],
    ) -> None:
        environment = dict(env)
        environment["GATEWAY_ID"] = gateway_id
        environment["GATEWAY_TYPE"] = gateway_type
        environment["GATEWAY_AGENT_ID"] = agent_id
        environment["GATEWAY_CLIENT_SERVICE_BASE_URL"] = self._client_service_url

        # Tear down any leftover container from a previous run with the same id.
        self._remove_container(container_name)

        self._client.containers.run(
            self._gateway_image,
            auto_remove=True,
            detach=True,
            entrypoint=[
                "/usr/local/bin/uv",
                "run",
                "--no-dev",
                "--package",
                "gateway-host",
                "-m",
                "gateway_host.api_main",
            ],
            environment=environment,
            labels={
                "agentspace.role": "gateway",
                "agentspace.gateway_id": gateway_id,
                "agentspace.gateway_type": gateway_type,
            },
            name=container_name,
            network=self._network,
        )

    def _docker_session(self, session: GatewayRuntimeSession) -> DockerGatewaySession:
        if not isinstance(session.value, DockerGatewaySession):
            msg = f"unsupported gateway session handle: {type(session.value)!r}"
            raise TypeError(msg)
        return session.value

    def _remove_container(self, container_name: str) -> None:
        try:
            container = self._client.containers.get(container_name)
        except NotFound:
            return
        container.remove(force=True)

    def _docker_container_logs(self, container_name: str) -> list[str]:
        try:
            container = self._client.containers.get(container_name)
        except NotFound:
            return []
        raw: bytes = container.logs(tail=200)
        return raw.decode(errors="replace").splitlines()

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
                    msg = f"gateway container at {base_url} did not become ready"
                    raise TimeoutError(msg)
                await asyncio.sleep(1)


class GatewayHost:
    def __init__(self, runtime: GatewayRuntime | None = None) -> None:
        self._runtime = runtime or DockerGatewayRuntime()
        self._gateways: dict[str, GatewayRecord] = {}
        self._lock = asyncio.Lock()

    async def create_gateway(
        self,
        *,
        gateway_id: str,
        gateway_type: str,
        agent_id: str,
        env: dict[str, str] | None = None,
    ) -> dict[str, object]:
        env = dict(env or {})
        async with self._lock:
            if gateway_id in self._gateways:
                raise GatewayAlreadyExistsError(gateway_id)
        runtime_session = await self._runtime.create_gateway(
            gateway_id=gateway_id,
            gateway_type=gateway_type,
            agent_id=agent_id,
            env=env,
        )
        record = GatewayRecord(
            gateway_id=gateway_id,
            gateway_type=gateway_type,
            agent_id=agent_id,
            runtime_session=runtime_session,
            env=env,
        )
        async with self._lock:
            self._gateways[gateway_id] = record
        return await self._summary_with_status(record)

    async def destroy_gateway(self, gateway_id: str) -> None:
        async with self._lock:
            record = self._gateways.pop(gateway_id, None)
        if record is None:
            raise GatewayNotFoundError(gateway_id)
        await self._runtime.destroy_gateway(session=record.runtime_session)

    async def destroy_all_gateways(self) -> None:
        async with self._lock:
            records = list(self._gateways.values())
            self._gateways.clear()
        for record in records:
            try:
                await self._runtime.destroy_gateway(session=record.runtime_session)
            except (OSError, DockerException, httpx.HTTPError):
                logger.warning(
                    "failed to destroy gateway %s",
                    record.gateway_id,
                    exc_info=True,
                )

    async def list_gateways(self) -> list[dict[str, object]]:
        async with self._lock:
            records = list(self._gateways.values())
        return [await self._summary_with_status(record) for record in records]

    async def get_gateway(self, gateway_id: str) -> dict[str, object]:
        record = self._require(gateway_id)
        return await self._summary_with_status(record)

    async def gateway_logs(self, gateway_id: str) -> list[str]:
        record = self._require(gateway_id)
        return await self._runtime.logs(session=record.runtime_session)

    async def _summary_with_status(self, record: GatewayRecord) -> dict[str, object]:
        status = await self._runtime.status(session=record.runtime_session)
        merged = record.summary()
        merged["status"] = str(status.get("status", "unknown"))
        merged["last_error"] = status.get("last_error")
        return merged

    def _require(self, gateway_id: str) -> GatewayRecord:
        try:
            return self._gateways[gateway_id]
        except KeyError as exc:
            raise GatewayNotFoundError(gateway_id) from exc

    # Test helper, not used by FastAPI routes.
    def _records(self) -> Iterable[GatewayRecord]:
        return list(self._gateways.values())
