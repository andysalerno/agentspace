"""Workspace persistence: protocol and SQLite/in-memory implementations."""

from __future__ import annotations

import sqlite3
from typing import TYPE_CHECKING, Protocol

from client_service.models import WorkspaceRecord

if TYPE_CHECKING:
    from client_service.storage.db import Database

WORKSPACES_SCHEMA = """
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"""


class WorkspaceExistsError(ValueError):
    pass


class WorkspaceMissingError(KeyError):
    pass


class WorkspaceStore(Protocol):
    async def list(self) -> list[WorkspaceRecord]: ...
    async def get(self, workspace_id: str) -> WorkspaceRecord | None: ...
    async def insert(self, workspace: WorkspaceRecord) -> None: ...
    async def update(self, workspace: WorkspaceRecord) -> None: ...
    async def delete(self, workspace_id: str) -> bool: ...


class InMemoryWorkspaceStore:
    def __init__(self) -> None:
        self._workspaces: dict[str, WorkspaceRecord] = {}

    async def list(self) -> list[WorkspaceRecord]:
        return list(self._workspaces.values())

    async def get(self, workspace_id: str) -> WorkspaceRecord | None:
        return self._workspaces.get(workspace_id)

    async def insert(self, workspace: WorkspaceRecord) -> None:
        if workspace.workspace_id in self._workspaces:
            raise WorkspaceExistsError(workspace.workspace_id)
        self._workspaces[workspace.workspace_id] = workspace

    async def update(self, workspace: WorkspaceRecord) -> None:
        if workspace.workspace_id not in self._workspaces:
            raise WorkspaceMissingError(workspace.workspace_id)
        self._workspaces[workspace.workspace_id] = workspace

    async def delete(self, workspace_id: str) -> bool:
        return self._workspaces.pop(workspace_id, None) is not None


class SqliteWorkspaceStore:
    def __init__(self, database: Database) -> None:
        self._db = database

    async def initialize(self) -> None:
        await self._db.executescript(WORKSPACES_SCHEMA)

    async def list(self) -> list[WorkspaceRecord]:
        rows = await self._db.fetch_all(
            "SELECT * FROM workspaces ORDER BY created_at ASC",
        )
        return [_row_to_workspace(row) for row in rows]

    async def get(self, workspace_id: str) -> WorkspaceRecord | None:
        row = await self._db.fetch_one(
            "SELECT * FROM workspaces WHERE workspace_id = ?",
            (workspace_id,),
        )
        return _row_to_workspace(row) if row is not None else None

    async def insert(self, workspace: WorkspaceRecord) -> None:
        try:
            await self._db.execute(
                """
                INSERT INTO workspaces (
                    workspace_id, name, created_at, updated_at
                ) VALUES (?, ?, ?, ?)
                """,
                (
                    workspace.workspace_id,
                    workspace.name,
                    workspace.created_at,
                    workspace.updated_at,
                ),
            )
        except sqlite3.IntegrityError as exc:
            raise WorkspaceExistsError(workspace.workspace_id) from exc

    async def update(self, workspace: WorkspaceRecord) -> None:
        existing = await self.get(workspace.workspace_id)
        if existing is None:
            raise WorkspaceMissingError(workspace.workspace_id)
        await self._db.execute(
            """
            UPDATE workspaces
               SET name = ?,
                   updated_at = ?
             WHERE workspace_id = ?
            """,
            (
                workspace.name,
                workspace.updated_at,
                workspace.workspace_id,
            ),
        )

    async def delete(self, workspace_id: str) -> bool:
        existing = await self.get(workspace_id)
        if existing is None:
            return False
        await self._db.execute(
            "DELETE FROM workspaces WHERE workspace_id = ?",
            (workspace_id,),
        )
        return True


def _row_to_workspace(row: object) -> WorkspaceRecord:
    mapping: dict[str, object] = dict(row)  # type: ignore[arg-type]
    return WorkspaceRecord(
        workspace_id=str(mapping["workspace_id"]),
        name=str(mapping["name"]),
        created_at=str(mapping["created_at"]),
        updated_at=str(mapping["updated_at"]),
    )
