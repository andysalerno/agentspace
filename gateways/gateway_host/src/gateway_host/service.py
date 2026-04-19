"""Gateway-host service — wires environment config to a Gateway implementation."""

from __future__ import annotations

import logging
import os
import tempfile
from collections import deque
from pathlib import Path
from typing import TYPE_CHECKING

from gateway.client import ClientServiceClient
from gateway.protocol import GatewayConfig, GatewayStatus, GatewayType

from gateway_host.registry import get_gateway

if TYPE_CHECKING:
    from gateway.protocol import Gateway

logger = logging.getLogger(__name__)

LOG_LIMIT = 500


class GatewayHostService:
    """Wraps a single gateway instance constructed from environment vars."""

    def __init__(self, *, gateway: Gateway, config: GatewayConfig) -> None:
        self._gateway = gateway
        self._config = config
        self._log_path = Path(tempfile.mkdtemp()) / "gateway.log"
        self._log_path.touch()
        self._log_buffer: deque[str] = deque(maxlen=LOG_LIMIT)

    @property
    def gateway(self) -> Gateway:
        return self._gateway

    async def start(self) -> None:
        try:
            await self._gateway.start(self._config)
        except Exception as exc:
            logger.exception("gateway %s failed to start", self._gateway.name)
            self._append_log(f"start error: {exc}")
            raise

    async def stop(self) -> None:
        try:
            await self._gateway.stop()
        except Exception as exc:  # noqa: BLE001 - shutdown should not raise
            logger.warning("gateway %s stop raised: %s", self._gateway.name, exc)
            self._append_log(f"stop error: {exc}")

    def status_summary(self) -> dict[str, object]:
        return {
            "type": self._gateway.name,
            "gateway_id": self._config.gateway_id,
            "agent_id": self._config.agent_id,
            "status": self._gateway.status.value,
            "last_error": self._gateway.last_error,
        }

    def logs(self) -> list[str]:
        return list(self._log_buffer)

    def _append_log(self, line: str) -> None:
        self._log_buffer.append(line)


def service_from_env() -> GatewayHostService:
    gateway_type_raw = os.environ.get("GATEWAY_TYPE")
    if not gateway_type_raw:
        msg = "GATEWAY_TYPE environment variable is required"
        raise RuntimeError(msg)
    gateway_id = os.environ.get("GATEWAY_ID")
    if not gateway_id:
        msg = "GATEWAY_ID environment variable is required"
        raise RuntimeError(msg)
    agent_id = os.environ.get("GATEWAY_AGENT_ID")
    if not agent_id:
        msg = "GATEWAY_AGENT_ID environment variable is required"
        raise RuntimeError(msg)
    base_url = os.environ.get(
        "GATEWAY_CLIENT_SERVICE_BASE_URL",
        "http://client-service:8002",
    )
    timeout = float(os.environ.get("GATEWAY_CLIENT_SERVICE_TIMEOUT", "60"))

    gateway_type = GatewayType(gateway_type_raw)
    gateway = get_gateway(gateway_type)
    client = ClientServiceClient(base_url=base_url, timeout=timeout)
    config = GatewayConfig(
        gateway_id=gateway_id,
        agent_id=agent_id,
        client=client,
        env=dict(os.environ),
    )
    service = GatewayHostService(gateway=gateway, config=config)
    logger.info(
        "constructed gateway host: type=%s gateway_id=%s agent_id=%s base_url=%s",
        gateway_type.value,
        gateway_id,
        agent_id,
        base_url,
    )
    return service


__all__ = ["GatewayHostService", "GatewayStatus", "service_from_env"]
