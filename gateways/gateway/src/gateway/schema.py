"""Per-type configuration schema for gateways.

The schema is consumed by ``client_service`` (served via
``GET /gateway-types/{type}/schema``) and by the webui to render labelled
inputs in the create/edit gateway form.

Schemas live in this shared package (rather than inside each gateway
implementation) so that ``client_service`` can serve them without
importing every gateway implementation directly.  Adding a new gateway
type therefore touches three places:

1. The ``GatewayType`` enum in :mod:`gateway.protocol`.
2. The schema entry in :data:`GATEWAY_SCHEMAS` below.
3. The implementation registered in ``gateway_host``.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Literal

from gateway.protocol import GatewayType

FieldKind = Literal["env", "secret"]


@dataclass(frozen=True, slots=True)
class GatewayConfigField:
    """Description of a single configuration field for a gateway type."""

    key: str
    label: str
    kind: FieldKind
    required: bool = False
    description: str | None = None
    default: str | None = None
    placeholder: str | None = None

    def to_dict(self) -> dict[str, object]:
        return {k: v for k, v in asdict(self).items() if v is not None}


@dataclass(frozen=True, slots=True)
class GatewaySchema:
    """All declared configuration fields for one gateway type."""

    fields: tuple[GatewayConfigField, ...] = field(default_factory=tuple)

    def to_dict(self) -> dict[str, object]:
        return {"fields": [f.to_dict() for f in self.fields]}


_DISCORD_SCHEMA = GatewaySchema(
    fields=(
        GatewayConfigField(
            key="DISCORD_BOT_TOKEN",
            label="Bot token",
            kind="secret",
            required=True,
            description="Bot token from the Discord Developer Portal.",
        ),
        GatewayConfigField(
            key="DISCORD_OWNER_USER_ID",
            label="Owner user ID",
            kind="env",
            required=True,
            description=(
                "Discord snowflake user ID of the only user the bot will "
                "respond to in DMs."
            ),
            placeholder="123456789012345678",
        ),
        GatewayConfigField(
            key="DISCORD_TYPING_DELAY_MS",
            label="Typing delay (ms)",
            kind="env",
            required=False,
            description=(
                "Delay between outbound message chunks; the typing indicator "
                "is shown during this delay."
            ),
            default="600",
        ),
        GatewayConfigField(
            key="DISCORD_CHUNK_MAX_CHARS",
            label="Max chunk size (chars)",
            kind="env",
            required=False,
            description=(
                "Maximum characters per outbound Discord message. "
                "Discord's hard limit is 2000."
            ),
            default="1900",
        ),
    ),
)


GATEWAY_SCHEMAS: dict[GatewayType, GatewaySchema] = {
    GatewayType.ECHO: GatewaySchema(),
    GatewayType.DISCORD: _DISCORD_SCHEMA,
}


def get_schema(gateway_type: GatewayType) -> GatewaySchema:
    return GATEWAY_SCHEMAS.get(gateway_type, GatewaySchema())


__all__ = [
    "GATEWAY_SCHEMAS",
    "GatewayConfigField",
    "GatewaySchema",
    "get_schema",
]
