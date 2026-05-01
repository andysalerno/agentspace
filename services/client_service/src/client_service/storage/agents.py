"""Agent persistence: protocol and SQLite/in-memory implementations."""

from __future__ import annotations

import json
import sqlite3
from typing import TYPE_CHECKING, Protocol, cast

from kernel_host.registry import HarnessName

from client_service.models import AgentRecord, WorkspaceMountMode, WorkspaceMountRecord

if TYPE_CHECKING:
    from client_service.storage.db import Database

AGENTS_SCHEMA = """
CREATE TABLE IF NOT EXISTS agents (
    agent_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    harness TEXT NOT NULL,
    system_prompt TEXT NOT NULL DEFAULT '',
    skills_json TEXT NOT NULL DEFAULT '[]',
    env_vars TEXT NOT NULL DEFAULT '',
    connection_id TEXT,
    workspace_mounts_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"""


class AgentExistsError(ValueError):
    pass


class AgentMissingError(KeyError):
    pass


class AgentStore(Protocol):
    async def list(self) -> list[AgentRecord]: ...
    async def get(self, agent_id: str) -> AgentRecord | None: ...
    async def insert(self, agent: AgentRecord) -> None: ...
    async def update(self, agent: AgentRecord) -> None: ...
    async def delete(self, agent_id: str) -> bool: ...


class InMemoryAgentStore:
    def __init__(self) -> None:
        self._agents: dict[str, AgentRecord] = {}

    async def list(self) -> list[AgentRecord]:
        return list(self._agents.values())

    async def get(self, agent_id: str) -> AgentRecord | None:
        return self._agents.get(agent_id)

    async def insert(self, agent: AgentRecord) -> None:
        if agent.agent_id in self._agents:
            raise AgentExistsError(agent.agent_id)
        self._agents[agent.agent_id] = agent

    async def update(self, agent: AgentRecord) -> None:
        if agent.agent_id not in self._agents:
            raise AgentMissingError(agent.agent_id)
        self._agents[agent.agent_id] = agent

    async def delete(self, agent_id: str) -> bool:
        return self._agents.pop(agent_id, None) is not None


class SqliteAgentStore:
    def __init__(self, database: Database) -> None:
        self._db = database

    async def initialize(self) -> None:
        await self._db.executescript(AGENTS_SCHEMA)
        await self._ensure_connection_id_column()
        await self._ensure_workspace_mounts_column()

    async def list(self) -> list[AgentRecord]:
        rows = await self._db.fetch_all(
            "SELECT * FROM agents ORDER BY created_at ASC",
        )
        return [_row_to_agent(row) for row in rows]

    async def get(self, agent_id: str) -> AgentRecord | None:
        row = await self._db.fetch_one(
            "SELECT * FROM agents WHERE agent_id = ?",
            (agent_id,),
        )
        return _row_to_agent(row) if row is not None else None

    async def insert(self, agent: AgentRecord) -> None:
        try:
            await self._db.execute(
                """
                INSERT INTO agents (
                    agent_id, name, harness, system_prompt,
                    skills_json, env_vars, connection_id, workspace_mounts_json,
                    created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    agent.agent_id,
                    agent.name,
                    agent.harness.value,
                    agent.system_prompt,
                    json.dumps(agent.skills),
                    agent.env_vars,
                    agent.connection_id,
                    json.dumps([mount.summary() for mount in agent.workspace_mounts]),
                    agent.created_at,
                    agent.updated_at,
                ),
            )
        except sqlite3.IntegrityError as exc:
            # The agents table has only a primary-key constraint, so any
            # IntegrityError on insert is a duplicate agent_id.
            raise AgentExistsError(agent.agent_id) from exc

    async def update(self, agent: AgentRecord) -> None:
        existing = await self.get(agent.agent_id)
        if existing is None:
            raise AgentMissingError(agent.agent_id)
        await self._db.execute(
            """
            UPDATE agents
               SET name = ?,
                   harness = ?,
                   system_prompt = ?,
                   skills_json = ?,
                   env_vars = ?,
                   connection_id = ?,
                   workspace_mounts_json = ?,
                   updated_at = ?
             WHERE agent_id = ?
            """,
            (
                agent.name,
                agent.harness.value,
                agent.system_prompt,
                json.dumps(agent.skills),
                agent.env_vars,
                agent.connection_id,
                json.dumps([mount.summary() for mount in agent.workspace_mounts]),
                agent.updated_at,
                agent.agent_id,
            ),
        )

    async def delete(self, agent_id: str) -> bool:
        existing = await self.get(agent_id)
        if existing is None:
            return False
        await self._db.execute(
            "DELETE FROM agents WHERE agent_id = ?",
            (agent_id,),
        )
        return True

    async def _ensure_connection_id_column(self) -> None:
        rows = await self._db.fetch_all("PRAGMA table_info(agents)")
        columns = {str(row["name"]) for row in rows}
        if "connection_id" not in columns:
            await self._db.execute("ALTER TABLE agents ADD COLUMN connection_id TEXT")

    async def _ensure_workspace_mounts_column(self) -> None:
        rows = await self._db.fetch_all("PRAGMA table_info(agents)")
        columns = {str(row["name"]) for row in rows}
        if "workspace_mounts_json" not in columns:
            await self._db.execute(
                "ALTER TABLE agents ADD COLUMN workspace_mounts_json "
                "TEXT NOT NULL DEFAULT '[]'",
            )


def _row_to_agent(row: object) -> AgentRecord:
    # row is sqlite3.Row, but typed as object to keep this module
    # importable without sqlite3 type stubs in scope.
    mapping: dict[str, object] = dict(row)  # type: ignore[arg-type]
    skills_raw = mapping["skills_json"]
    decoded: object = json.loads(str(skills_raw)) if skills_raw else []
    items = cast("list[object]", decoded) if isinstance(decoded, list) else []
    skills: list[str] = [str(item) for item in items]
    raw_connection = mapping["connection_id"]
    connection_id = None if raw_connection is None else str(raw_connection)
    mounts_raw = mapping.get("workspace_mounts_json", "[]")
    return AgentRecord(
        agent_id=str(mapping["agent_id"]),
        name=str(mapping["name"]),
        harness=HarnessName(str(mapping["harness"])),
        system_prompt=str(mapping["system_prompt"]),
        skills=skills,
        env_vars=str(mapping["env_vars"]),
        connection_id=connection_id,
        workspace_mounts=_workspace_mounts_from_json(str(mounts_raw)),
        created_at=str(mapping["created_at"]),
        updated_at=str(mapping["updated_at"]),
    )


def _workspace_mounts_from_json(raw: str) -> list[WorkspaceMountRecord]:
    decoded: object = json.loads(raw) if raw else []
    items = cast("list[object]", decoded) if isinstance(decoded, list) else []
    mounts: list[WorkspaceMountRecord] = []
    seen: set[str] = set()
    for item in items:
        if not isinstance(item, dict):
            continue
        mapping = cast("dict[object, object]", item)
        workspace_id = mapping.get("workspace_id")
        if (
            not isinstance(workspace_id, str)
            or not workspace_id
            or workspace_id in seen
        ):
            continue
        mode_raw = str(mapping.get("mode") or WorkspaceMountMode.READ_WRITE.value)
        mode = (
            WorkspaceMountMode.READ_ONLY
            if mode_raw == WorkspaceMountMode.READ_ONLY.value
            else WorkspaceMountMode.READ_WRITE
        )
        mounts.append(WorkspaceMountRecord(workspace_id=workspace_id, mode=mode))
        seen.add(workspace_id)
    return mounts
