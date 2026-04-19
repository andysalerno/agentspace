"""Gateway registry — maps gateway type names to gateway classes."""

from __future__ import annotations

from typing import TYPE_CHECKING

from gateway.protocol import GatewayType
from gateway_discord import DiscordGateway
from gateway_echo import EchoGateway

if TYPE_CHECKING:
    from gateway.protocol import Gateway


GATEWAY_REGISTRY: dict[GatewayType, type] = {
    GatewayType.ECHO: EchoGateway,
    GatewayType.DISCORD: DiscordGateway,
}


def get_gateway(gateway_type: GatewayType) -> Gateway:
    cls = GATEWAY_REGISTRY.get(gateway_type)
    if cls is None:
        available = ", ".join(sorted(name.value for name in GATEWAY_REGISTRY))
        msg = f"Unknown gateway type: {gateway_type!r}. Available: {available}"
        raise ValueError(msg)
    return cls()  # type: ignore[return-value]
