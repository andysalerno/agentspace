"""Gateway protocol — the type contract every gateway implementation satisfies."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import StrEnum
from typing import TYPE_CHECKING, Protocol, runtime_checkable

if TYPE_CHECKING:
    from fastapi import APIRouter

    from gateway.client import ClientServiceClient


class GatewayType(StrEnum):
    ECHO = "echo"


class GatewayStatus(StrEnum):
    STOPPED = "stopped"
    STARTING = "starting"
    RUNNING = "running"
    ERROR = "error"


def _empty_env() -> dict[str, str]:
    return {}


@dataclass(frozen=True, slots=True)
class GatewayConfig:
    """Configuration handed to a gateway implementation on start."""

    gateway_id: str
    agent_id: str
    client: ClientServiceClient
    env: dict[str, str] = field(default_factory=_empty_env)


@runtime_checkable
class Gateway(Protocol):
    """Structural type contract for a gateway implementation.

    A gateway is a long-running process that bridges some external system
    (Discord, Slack, an HTTP inbox, ...) to an AgentSpace agent by talking
    to the client_service API on its behalf.
    """

    @property
    def name(self) -> str: ...

    @property
    def status(self) -> GatewayStatus: ...

    @property
    def last_error(self) -> str | None: ...

    async def start(self, config: GatewayConfig) -> None:
        """Begin servicing the external system.

        Implementations should return promptly; long-running work belongs
        in background tasks.  ``status`` should transition to ``RUNNING``
        before this call returns (or to ``ERROR`` with ``last_error`` set).
        """

    async def stop(self) -> None:
        """Tear down background tasks and external connections."""

    def extra_router(self) -> APIRouter | None:
        """Return an optional FastAPI router with gateway-specific routes.

        Mounted by the gateway_host under ``/gateway`` at startup.
        """
