"""Gateway abstraction — types and helpers shared by all gateway implementations."""

from gateway.client import ClientServiceClient, ClientServiceError
from gateway.events import GatewayEvent, GatewayEventType
from gateway.protocol import (
    Gateway,
    GatewayConfig,
    GatewayStatus,
    GatewayType,
)
from gateway.schema import (
    GATEWAY_SCHEMAS,
    GatewayConfigField,
    GatewaySchema,
    get_schema,
)
from gateway.simulated_typing import (
    SimulatedTypingConfig,
    TypingChunk,
    plan_simulated_typing,
)

__all__ = [
    "GATEWAY_SCHEMAS",
    "ClientServiceClient",
    "ClientServiceError",
    "Gateway",
    "GatewayConfig",
    "GatewayConfigField",
    "GatewayEvent",
    "GatewayEventType",
    "GatewaySchema",
    "GatewayStatus",
    "GatewayType",
    "SimulatedTypingConfig",
    "TypingChunk",
    "get_schema",
    "plan_simulated_typing",
]
