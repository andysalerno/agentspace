from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from client_service.models import AgentRecord
from client_service.storage import (
    AgentStore,
    Database,
    InMemoryAgentStore,
    SqliteAgentStore,
)
from client_service.storage.agents import AgentExistsError, AgentMissingError
from kernel_host.registry import HarnessName

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Callable
    from pathlib import Path

    StoreFactory = Callable[[Path], AsyncIterator[AgentStore]]


def _make_agent(agent_id: str = "agent-one", **overrides: object) -> AgentRecord:
    defaults: dict[str, object] = {
        "agent_id": agent_id,
        "name": "Agent One",
        "harness": HarnessName.COPILOT_CLI,
        "system_prompt": "be helpful",
        "skills": ["alpha", "beta"],
        "env_vars": "FOO=bar\nBAZ=qux",
    }
    defaults.update(overrides)
    return AgentRecord(**defaults)  # type: ignore[arg-type]


async def _open_in_memory(_tmp_path: Path) -> AsyncIterator[AgentStore]:
    yield InMemoryAgentStore()


async def _open_sqlite(tmp_path: Path) -> AsyncIterator[AgentStore]:
    db = Database(tmp_path / "test.sqlite")
    await db.connect()
    store = SqliteAgentStore(db)
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
async def test_insert_and_get_roundtrip(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        agent = _make_agent()
        await store.insert(agent)
        fetched = await store.get(agent.agent_id)
        assert fetched is not None
        assert fetched.agent_id == agent.agent_id
        assert fetched.name == agent.name
        assert fetched.harness == agent.harness
        assert fetched.system_prompt == agent.system_prompt
        assert fetched.skills == agent.skills
        assert fetched.env_vars == agent.env_vars
        assert fetched.created_at == agent.created_at
        assert fetched.updated_at == agent.updated_at


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_get_missing_returns_none(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        assert await store.get("nope") is None


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_insert_duplicate_raises(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        agent = _make_agent()
        await store.insert(agent)
        with pytest.raises(AgentExistsError):
            await store.insert(agent)


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_list_returns_inserted_agents(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        a1 = _make_agent("agent-one")
        a2 = _make_agent("agent-two", name="Agent Two")
        await store.insert(a1)
        await store.insert(a2)
        listed = sorted(await store.list(), key=lambda a: a.agent_id)
        assert [a.agent_id for a in listed] == ["agent-one", "agent-two"]


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_update_changes_fields(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        agent = _make_agent()
        await store.insert(agent)
        agent.name = "Renamed"
        agent.skills = ["only-one"]
        agent.env_vars = ""
        agent.updated_at = "2026-04-18T00:00:00+00:00"
        await store.update(agent)
        fetched = await store.get(agent.agent_id)
        assert fetched is not None
        assert fetched.name == "Renamed"
        assert fetched.skills == ["only-one"]
        assert fetched.env_vars == ""
        assert fetched.updated_at == "2026-04-18T00:00:00+00:00"


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_update_missing_raises(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        with pytest.raises(AgentMissingError):
            await store.update(_make_agent("ghost"))


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_delete_returns_true_when_present(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        agent = _make_agent()
        await store.insert(agent)
        assert await store.delete(agent.agent_id) is True
        assert await store.get(agent.agent_id) is None


@pytest.mark.parametrize("factory", STORE_FACTORIES)
async def test_delete_missing_returns_false(
    factory: StoreFactory,
    tmp_path: Path,
) -> None:
    async for store in factory(tmp_path):
        assert await store.delete("ghost") is False


async def test_sqlite_persists_across_connections(tmp_path: Path) -> None:
    db_path = tmp_path / "persist.sqlite"

    db1 = Database(db_path)
    await db1.connect()
    store1 = SqliteAgentStore(db1)
    await store1.initialize()
    await store1.insert(_make_agent("durable"))
    await db1.close()

    db2 = Database(db_path)
    await db2.connect()
    store2 = SqliteAgentStore(db2)
    await store2.initialize()
    fetched = await store2.get("durable")
    await db2.close()

    assert fetched is not None
    assert fetched.agent_id == "durable"
    assert fetched.skills == ["alpha", "beta"]


async def test_sqlite_initialize_migrates_missing_connection_id(
    tmp_path: Path,
) -> None:
    db = Database(tmp_path / "old-schema.sqlite")
    await db.connect()
    await db.executescript(
        """
        CREATE TABLE agents (
            agent_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            harness TEXT NOT NULL,
            system_prompt TEXT NOT NULL DEFAULT '',
            skills_json TEXT NOT NULL DEFAULT '[]',
            env_vars TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        """,
    )

    store = SqliteAgentStore(db)
    await store.initialize()
    agent = _make_agent("migrated")
    await store.insert(agent)
    fetched = await store.get("migrated")
    await db.close()

    assert fetched is not None
    assert fetched.connection_id is None
