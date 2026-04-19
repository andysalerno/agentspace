"""Gateway persistence: protocol and SQLite/in-memory implementations."""

from __future__ import annotations

import json
import sqlite3
from typing import TYPE_CHECKING, Protocol, cast

from gateway.protocol import GatewayType

from client_service.models import GatewayRecord

if TYPE_CHECKING:
    from client_service.storage.db import Database

GATEWAYS_SCHEMA = """
CREATE TABLE IF NOT EXISTS gateways (
    gateway_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    gateway_type TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 0,
    env_vars TEXT NOT NULL DEFAULT '',
    secrets_json TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'stopped',
    last_error TEXT,
    container_name TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"""


class GatewayExistsError(ValueError):
    pass


class GatewayMissingError(KeyError):
    pass


class GatewayStore(Protocol):
    async def list(self) -> list[GatewayRecord]: ...
    async def get(self, gateway_id: str) -> GatewayRecord | None: ...
    async def insert(self, gateway: GatewayRecord) -> None: ...
    async def update(self, gateway: GatewayRecord) -> None: ...
    async def delete(self, gateway_id: str) -> bool: ...


class InMemoryGatewayStore:
    def __init__(self) -> None:
        self._gateways: dict[str, GatewayRecord] = {}

    async def list(self) -> list[GatewayRecord]:
        return list(self._gateways.values())

    async def get(self, gateway_id: str) -> GatewayRecord | None:
        return self._gateways.get(gateway_id)

    async def insert(self, gateway: GatewayRecord) -> None:
        if gateway.gateway_id in self._gateways:
            raise GatewayExistsError(gateway.gateway_id)
        self._gateways[gateway.gateway_id] = gateway

    async def update(self, gateway: GatewayRecord) -> None:
        if gateway.gateway_id not in self._gateways:
            raise GatewayMissingError(gateway.gateway_id)
        self._gateways[gateway.gateway_id] = gateway

    async def delete(self, gateway_id: str) -> bool:
        return self._gateways.pop(gateway_id, None) is not None


class SqliteGatewayStore:
    def __init__(self, database: Database) -> None:
        self._db = database

    async def initialize(self) -> None:
        await self._db.executescript(GATEWAYS_SCHEMA)

    async def list(self) -> list[GatewayRecord]:
        rows = await self._db.fetch_all(
            "SELECT * FROM gateways ORDER BY created_at ASC",
        )
        return [_row_to_gateway(row) for row in rows]

    async def get(self, gateway_id: str) -> GatewayRecord | None:
        row = await self._db.fetch_one(
            "SELECT * FROM gateways WHERE gateway_id = ?",
            (gateway_id,),
        )
        return _row_to_gateway(row) if row is not None else None

    async def insert(self, gateway: GatewayRecord) -> None:
        try:
            await self._db.execute(
                """
                INSERT INTO gateways (
                    gateway_id, name, gateway_type, agent_id, enabled,
                    env_vars, secrets_json, status, last_error,
                    container_name, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    gateway.gateway_id,
                    gateway.name,
                    gateway.gateway_type.value,
                    gateway.agent_id,
                    1 if gateway.enabled else 0,
                    gateway.env_vars,
                    json.dumps(gateway.secrets),
                    gateway.status,
                    gateway.last_error,
                    gateway.container_name,
                    gateway.created_at,
                    gateway.updated_at,
                ),
            )
        except sqlite3.IntegrityError as exc:
            raise GatewayExistsError(gateway.gateway_id) from exc

    async def update(self, gateway: GatewayRecord) -> None:
        existing = await self.get(gateway.gateway_id)
        if existing is None:
            raise GatewayMissingError(gateway.gateway_id)
        await self._db.execute(
            """
            UPDATE gateways
               SET name = ?,
                   gateway_type = ?,
                   agent_id = ?,
                   enabled = ?,
                   env_vars = ?,
                   secrets_json = ?,
                   status = ?,
                   last_error = ?,
                   container_name = ?,
                   updated_at = ?
             WHERE gateway_id = ?
            """,
            (
                gateway.name,
                gateway.gateway_type.value,
                gateway.agent_id,
                1 if gateway.enabled else 0,
                gateway.env_vars,
                json.dumps(gateway.secrets),
                gateway.status,
                gateway.last_error,
                gateway.container_name,
                gateway.updated_at,
                gateway.gateway_id,
            ),
        )

    async def delete(self, gateway_id: str) -> bool:
        existing = await self.get(gateway_id)
        if existing is None:
            return False
        await self._db.execute(
            "DELETE FROM gateways WHERE gateway_id = ?",
            (gateway_id,),
        )
        return True


def _row_to_gateway(row: object) -> GatewayRecord:
    mapping: dict[str, object] = dict(row)  # type: ignore[arg-type]
    secrets_raw = mapping["secrets_json"]
    decoded: object = json.loads(str(secrets_raw)) if secrets_raw else {}
    secrets: dict[str, str] = {}
    if isinstance(decoded, dict):
        for key, value in cast("dict[object, object]", decoded).items():
            secrets[str(key)] = str(value)
    last_error_raw = mapping["last_error"]
    container_name_raw = mapping["container_name"]
    return GatewayRecord(
        gateway_id=str(mapping["gateway_id"]),
        name=str(mapping["name"]),
        gateway_type=GatewayType(str(mapping["gateway_type"])),
        agent_id=str(mapping["agent_id"]),
        enabled=bool(int(mapping["enabled"])),  # type: ignore[arg-type]
        env_vars=str(mapping["env_vars"]),
        secrets=secrets,
        status=str(mapping["status"]),
        last_error=None if last_error_raw is None else str(last_error_raw),
        container_name=None if container_name_raw is None else str(container_name_raw),
        created_at=str(mapping["created_at"]),
        updated_at=str(mapping["updated_at"]),
    )
