"""Gateway abstraction — types and helpers shared by all gateway implementations."""

from gateway.client import ClientServiceClient, ClientServiceError
from gateway.events import GatewayEvent, GatewayEventType
from gateway.protocol import (
    Gateway,
    GatewayConfig,
    GatewayStatus,
    GatewayType,
)

__all__ = [
    "ClientServiceClient",
    "ClientServiceError",
    "Gateway",
    "GatewayConfig",
    "GatewayEvent",
    "GatewayEventType",
    "GatewayStatus",
    "GatewayType",
]
