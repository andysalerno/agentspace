"""Integration tests wiring ClientService through SqliteAgentStore."""

from __future__ import annotations

from typing import TYPE_CHECKING, cast

import pytest
from client_service.service import (
    AgentAlreadyExistsError,
    AgentNotFoundError,
    ClientService,
)
from client_service.storage import Database, SqliteAgentStore
from kernel_host.registry import HarnessName

# Re-use the stub from the unit-test module rather than redefining it.
from .test_service import StubAgentHostClient

if TYPE_CHECKING:
    from pathlib import Path

    from client_service.agent_host_client import AgentHostClient


async def _make_service(
    db_path: Path,
) -> tuple[ClientService, Database]:
    db = Database(db_path)
    await db.connect()
    store = SqliteAgentStore(db)
    await store.initialize()
    runtime = cast("AgentHostClient", StubAgentHostClient())
    return ClientService(agent_host_client=runtime, agent_store=store), db


async def test_sqlite_backed_service_full_agent_lifecycle(tmp_path: Path) -> None:
    db_path = tmp_path / "service.sqlite"
    service, db = await _make_service(db_path)
    try:
        created = await service.create_agent(
            agent_id="durable-agent",
            name="Durable",
            harness=HarnessName.COPILOT_CLI,
            system_prompt="be helpful",
            skills=["alpha"],
            env_vars="FOO=bar",
        )
        assert created["agent_id"] == "durable-agent"

        with pytest.raises(AgentAlreadyExistsError):
            await service.create_agent(agent_id="durable-agent", name="Dup")

        listed = await service.list_agents()
        assert [a["agent_id"] for a in listed] == ["durable-agent"]

        updated = await service.update_agent(
            "durable-agent",
            name="Renamed",
            harness=None,
            system_prompt=None,
            skills=["alpha", "beta"],
            env_vars=None,
        )
        assert updated["name"] == "Renamed"
        assert updated["skills"] == ["alpha", "beta"]

        await service.delete_agent("durable-agent")
        with pytest.raises(AgentNotFoundError):
            await service.get_agent("durable-agent")
    finally:
        await db.close()


async def test_sqlite_backed_service_persists_across_instances(
    tmp_path: Path,
) -> None:
    db_path = tmp_path / "persist.sqlite"

    service1, db1 = await _make_service(db_path)
    try:
        await service1.create_agent(
            agent_id="survivor",
            name="Survivor",
            skills=["s1"],
            env_vars="K=V",
        )
    finally:
        await db1.close()

    service2, db2 = await _make_service(db_path)
    try:
        fetched = await service2.get_agent("survivor")
        assert fetched["agent_id"] == "survivor"
        assert fetched["name"] == "Survivor"
        assert fetched["skills"] == ["s1"]
        assert fetched["env_vars"] == "K=V"

        listed = await service2.list_agents()
        assert [a["agent_id"] for a in listed] == ["survivor"]
    finally:
        await db2.close()
