from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from client_service.models import WorkspaceRecord
from client_service.storage import (
    Database,
    InMemoryWorkspaceStore,
    SqliteWorkspaceStore,
    WorkspaceStore,
)
from client_service.storage.workspaces import (
    WorkspaceExistsError,
    WorkspaceMissingError,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Callable
    from pathlib import Path

    StoreFactory = Callable[[Path], AsyncIterator[WorkspaceStore]]


async def _open_in_memory(_tmp_path: Path) -> AsyncIterator[WorkspaceStore]:
    yield InMemoryWorkspaceStore()


async def _open_sqlite(tmp_path: Path) -> AsyncIterator[WorkspaceStore]:
    db = Database(tmp_path / "workspaces.sqlite")
    await db.connect()
    store = SqliteWorkspaceStore(db)
    await store.initialize()
    try:
        yield store
    finally:
        await db.close()


STORE_FACTORIES = [
    pytest.param(_open_in_memory, id="in_memory"),
    pytest.param(_open_sqlite, id="sqlite"),
]


def _make_workspace(workspace_id: str = "todo-list-code") -> WorkspaceRecord:
    return WorkspaceRecord(workspace_id=workspace_id, name="TodoListCode")


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_insert_and_get_roundtrip(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        workspace = _make_workspace()
        await store.insert(workspace)
        fetched = await store.get(workspace.workspace_id)
        assert fetched is not None
        assert fetched.workspace_id == workspace.workspace_id
        assert fetched.name == workspace.name
        assert fetched.volume_name == "agentspace-workspace-todo-list-code"


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_insert_duplicate_raises(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        workspace = _make_workspace()
        await store.insert(workspace)
        with pytest.raises(WorkspaceExistsError):
            await store.insert(workspace)


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_update_changes_name(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        workspace = _make_workspace()
        await store.insert(workspace)
        workspace.name = "Renamed"
        await store.update(workspace)
        fetched = await store.get(workspace.workspace_id)
        assert fetched is not None
        assert fetched.name == "Renamed"


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_update_missing_raises(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        with pytest.raises(WorkspaceMissingError):
            await store.update(_make_workspace("ghost"))


async def test_sqlite_persists_across_connections(tmp_path: Path) -> None:
    db_path = tmp_path / "persist-workspaces.sqlite"

    db1 = Database(db_path)
    await db1.connect()
    store1 = SqliteWorkspaceStore(db1)
    await store1.initialize()
    await store1.insert(_make_workspace("durable"))
    await db1.close()

    db2 = Database(db_path)
    await db2.connect()
    store2 = SqliteWorkspaceStore(db2)
    await store2.initialize()
    fetched = await store2.get("durable")
    await db2.close()

    assert fetched is not None
    assert fetched.workspace_id == "durable"
