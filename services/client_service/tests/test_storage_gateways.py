from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from client_service.models import GatewayRecord
from client_service.storage import (
    Database,
    GatewayStore,
    InMemoryGatewayStore,
    SqliteGatewayStore,
)
from client_service.storage.gateways import GatewayExistsError, GatewayMissingError
from gateway.protocol import GatewayType

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Callable
    from pathlib import Path

    StoreFactory = Callable[[Path], AsyncIterator[GatewayStore]]


def _make_gateway(
    gateway_id: str = "echo-bridge",
    **overrides: object,
) -> GatewayRecord:
    defaults: dict[str, object] = {
        "gateway_id": gateway_id,
        "name": "Echo Bridge",
        "gateway_type": GatewayType.ECHO,
        "agent_id": "agent-one",
        "enabled": True,
        "env_vars": "ECHO_TOKEN=abc",
        "secrets": {"DISCORD_TOKEN": "secret"},
    }
    defaults.update(overrides)
    return GatewayRecord(**defaults)  # type: ignore[arg-type]


async def _open_in_memory(_tmp_path: Path) -> AsyncIterator[GatewayStore]:
    yield InMemoryGatewayStore()


async def _open_sqlite(tmp_path: Path) -> AsyncIterator[GatewayStore]:
    db = Database(tmp_path / "gateways.sqlite")
    await db.connect()
    store = SqliteGatewayStore(db)
    await store.initialize()
    try:
        yield store
    finally:
        await db.close()


STORE_FACTORIES = [
    pytest.param(_open_in_memory, id="in_memory"),
    pytest.param(_open_sqlite, id="sqlite"),
]


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_insert_and_get(factory: StoreFactory, tmp_path: Path) -> None:
    async for store in factory(tmp_path):
        record = _make_gateway()
        await store.insert(record)
        loaded = await store.get(record.gateway_id)
        assert loaded is not None
        assert loaded.gateway_type == GatewayType.ECHO
        assert loaded.secrets == {"DISCORD_TOKEN": "secret"}
        assert loaded.enabled is True


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_insert_duplicate_raises(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        await store.insert(_make_gateway())
        with pytest.raises(GatewayExistsError):
            await store.insert(_make_gateway())


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_update_and_delete(factory: StoreFactory, tmp_path: Path) -> None:
    async for store in factory(tmp_path):
        record = _make_gateway()
        await store.insert(record)
        record.status = "running"
        record.container_name = "agentspace-gateway-echo-bridge"
        await store.update(record)
        loaded = await store.get(record.gateway_id)
        assert loaded is not None
        assert loaded.status == "running"
        assert loaded.container_name == "agentspace-gateway-echo-bridge"

        deleted = await store.delete(record.gateway_id)
        assert deleted is True
        assert await store.get(record.gateway_id) is None


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_update_missing_raises(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        with pytest.raises(GatewayMissingError):
            await store.update(_make_gateway())
