from __future__ import annotations

from gateway.protocol import GatewayType
from gateway.schema import GATEWAY_SCHEMAS


def test_every_gateway_type_has_a_schema() -> None:
    """Every gateway type has a schema.

    Adding a GatewayType without a matching GATEWAY_SCHEMAS entry would
    silently produce an empty schema and a broken create form. Catch it
    here at unit-test time instead.
    """
    missing = [gt for gt in GatewayType if gt not in GATEWAY_SCHEMAS]
    assert not missing, f"Missing schema entries for: {missing}"
