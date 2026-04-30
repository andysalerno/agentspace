"""Connection persistence: protocol and SQLite/in-memory implementations."""

from __future__ import annotations

import sqlite3
from typing import TYPE_CHECKING, Protocol, cast

from client_service.models import (
    DEFAULT_CONNECTION_API_FLAVOR,
    ConnectionApiFlavor,
    ConnectionRecord,
)

if TYPE_CHECKING:
    from client_service.storage.db import Database

CONNECTIONS_SCHEMA = """
CREATE TABLE IF NOT EXISTS connections (
    connection_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    api_flavor TEXT NOT NULL DEFAULT 'chat_completions',
    api_key TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"""


class ConnectionExistsError(ValueError):
    pass


class ConnectionMissingError(KeyError):
    pass


class ConnectionStore(Protocol):
    async def list(self) -> list[ConnectionRecord]: ...
    async def get(self, connection_id: str) -> ConnectionRecord | None: ...
    async def insert(self, connection: ConnectionRecord) -> None: ...
    async def update(self, connection: ConnectionRecord) -> None: ...
    async def delete(self, connection_id: str) -> bool: ...


class InMemoryConnectionStore:
    def __init__(self) -> None:
        self._connections: dict[str, ConnectionRecord] = {}

    async def list(self) -> list[ConnectionRecord]:
        return list(self._connections.values())

    async def get(self, connection_id: str) -> ConnectionRecord | None:
        return self._connections.get(connection_id)

    async def insert(self, connection: ConnectionRecord) -> None:
        if connection.connection_id in self._connections:
            raise ConnectionExistsError(connection.connection_id)
        self._connections[connection.connection_id] = connection

    async def update(self, connection: ConnectionRecord) -> None:
        if connection.connection_id not in self._connections:
            raise ConnectionMissingError(connection.connection_id)
        self._connections[connection.connection_id] = connection

    async def delete(self, connection_id: str) -> bool:
        return self._connections.pop(connection_id, None) is not None


class SqliteConnectionStore:
    def __init__(self, database: Database) -> None:
        self._db = database

    async def initialize(self) -> None:
        await self._db.executescript(CONNECTIONS_SCHEMA)
        await self._ensure_api_flavor_column()

    async def list(self) -> list[ConnectionRecord]:
        rows = await self._db.fetch_all(
            "SELECT * FROM connections ORDER BY created_at ASC",
        )
        return [_row_to_connection(row) for row in rows]

    async def get(self, connection_id: str) -> ConnectionRecord | None:
        row = await self._db.fetch_one(
            "SELECT * FROM connections WHERE connection_id = ?",
            (connection_id,),
        )
        return _row_to_connection(row) if row is not None else None

    async def insert(self, connection: ConnectionRecord) -> None:
        try:
            await self._db.execute(
                """
                INSERT INTO connections (
                    connection_id, name, url, api_flavor, api_key,
                    created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    connection.connection_id,
                    connection.name,
                    connection.url,
                    connection.api_flavor,
                    connection.api_key,
                    connection.created_at,
                    connection.updated_at,
                ),
            )
        except sqlite3.IntegrityError as exc:
            raise ConnectionExistsError(connection.connection_id) from exc

    async def update(self, connection: ConnectionRecord) -> None:
        existing = await self.get(connection.connection_id)
        if existing is None:
            raise ConnectionMissingError(connection.connection_id)
        await self._db.execute(
            """
            UPDATE connections
               SET name = ?,
                   url = ?,
                   api_flavor = ?,
                   api_key = ?,
                   updated_at = ?
             WHERE connection_id = ?
            """,
            (
                connection.name,
                connection.url,
                connection.api_flavor,
                connection.api_key,
                connection.updated_at,
                connection.connection_id,
            ),
        )

    async def delete(self, connection_id: str) -> bool:
        existing = await self.get(connection_id)
        if existing is None:
            return False
        await self._db.execute(
            "DELETE FROM connections WHERE connection_id = ?",
            (connection_id,),
        )
        return True

    async def _ensure_api_flavor_column(self) -> None:
        rows = await self._db.fetch_all("PRAGMA table_info(connections)")
        columns = {str(row["name"]) for row in rows}
        if "api_flavor" not in columns:
            await self._db.execute(
                "ALTER TABLE connections ADD COLUMN api_flavor TEXT NOT NULL "
                f"DEFAULT '{DEFAULT_CONNECTION_API_FLAVOR}'",
            )


def _row_to_connection(row: object) -> ConnectionRecord:
    mapping: dict[str, object] = dict(row)  # type: ignore[arg-type]
    return ConnectionRecord(
        connection_id=str(mapping["connection_id"]),
        name=str(mapping["name"]),
        url=str(mapping["url"]),
        api_flavor=_connection_api_flavor(mapping.get("api_flavor")),
        api_key=str(mapping["api_key"]),
        created_at=str(mapping["created_at"]),
        updated_at=str(mapping["updated_at"]),
    )


def _connection_api_flavor(value: object) -> ConnectionApiFlavor:
    api_flavor = str(value or DEFAULT_CONNECTION_API_FLAVOR)
    if api_flavor in {"chat_completions", "responses"}:
        return cast("ConnectionApiFlavor", api_flavor)
    return DEFAULT_CONNECTION_API_FLAVOR
