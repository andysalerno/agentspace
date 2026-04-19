"""Echo gateway — a reference gateway with no external dependencies.

It exposes a tiny HTTP API on the gateway_host that simulates an external
platform: ``POST /gateway/inbox`` accepts a message; the gateway forwards
it to client_service and stores the assistant reply in an in-memory
outbox retrievable via ``GET /gateway/outbox``.
"""

from gateway_echo.echo import EchoGateway

__all__ = ["EchoGateway"]
