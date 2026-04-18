from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from client_service.storage import (
    Database,
    InMemoryKernelConfigStore,
    KernelConfigStore,
    SqliteKernelConfigStore,
)
from kernel_host.registry import HarnessName

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Callable
    from pathlib import Path

    StoreFactory = Callable[[Path], AsyncIterator[KernelConfigStore]]


async def _open_in_memory(_tmp_path: Path) -> AsyncIterator[KernelConfigStore]:
    yield InMemoryKernelConfigStore()


async def _open_sqlite(tmp_path: Path) -> AsyncIterator[KernelConfigStore]:
    db = Database(tmp_path / "test.sqlite")
    await db.connect()
    store = SqliteKernelConfigStore(db)
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
async def test_get_missing_returns_none(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        assert await store.get(HarnessName.OPENCODE) is None


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_upsert_and_get_roundtrip(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        record = await store.upsert(
            HarnessName.OPENCODE,
            "OPENCODE_MODEL=gpt-5\nOPENCODE_AGENT=plan",
        )
        assert record.harness == HarnessName.OPENCODE
        assert record.env_vars == "OPENCODE_MODEL=gpt-5\nOPENCODE_AGENT=plan"

        fetched = await store.get(HarnessName.OPENCODE)
        assert fetched is not None
        assert fetched.env_vars == record.env_vars
        assert fetched.updated_at == record.updated_at


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_upsert_overwrites_existing(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        await store.upsert(HarnessName.OPENCODE, "OPENCODE_MODEL=old")
        await store.upsert(HarnessName.OPENCODE, "OPENCODE_MODEL=new")

        fetched = await store.get(HarnessName.OPENCODE)
        assert fetched is not None
        assert fetched.env_vars == "OPENCODE_MODEL=new"


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_list_returns_all_records(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        await store.upsert(HarnessName.OPENCODE, "A=1")
        await store.upsert(HarnessName.CODEX, "B=2")

        records = await store.list()
        harnesses = {record.harness for record in records}
        assert harnesses == {HarnessName.OPENCODE, HarnessName.CODEX}
