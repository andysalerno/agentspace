"""Kernel config persistence: protocol and SQLite/in-memory implementations.

A kernel config is a per-harness default ``env_vars`` blob used to prefill the
agent creation form in clients.  It is also merged into the session env at
session-create time, with per-agent ``env_vars`` taking precedence over the
per-harness defaults.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Protocol

from kernel_host.registry import HarnessName

from client_service.models import KernelConfigRecord, utc_now

if TYPE_CHECKING:
    from client_service.storage.db import Database

KERNEL_CONFIGS_SCHEMA = """
CREATE TABLE IF NOT EXISTS kernel_configs (
    harness TEXT PRIMARY KEY,
    env_vars TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);
"""


class KernelConfigStore(Protocol):
    async def list(self) -> list[KernelConfigRecord]: ...
    async def get(self, harness: HarnessName) -> KernelConfigRecord | None: ...
    async def upsert(
        self,
        harness: HarnessName,
        env_vars: str,
    ) -> KernelConfigRecord: ...


class InMemoryKernelConfigStore:
    def __init__(self) -> None:
        self._configs: dict[HarnessName, KernelConfigRecord] = {}

    async def list(self) -> list[KernelConfigRecord]:
        return list(self._configs.values())

    async def get(self, harness: HarnessName) -> KernelConfigRecord | None:
        return self._configs.get(harness)

    async def upsert(
        self,
        harness: HarnessName,
        env_vars: str,
    ) -> KernelConfigRecord:
        record = KernelConfigRecord(
            harness=harness,
            env_vars=env_vars,
            updated_at=utc_now(),
        )
        self._configs[harness] = record
        return record


class SqliteKernelConfigStore:
    def __init__(self, database: Database) -> None:
        self._db = database

    async def initialize(self) -> None:
        await self._db.executescript(KERNEL_CONFIGS_SCHEMA)

    async def list(self) -> list[KernelConfigRecord]:
        rows = await self._db.fetch_all(
            "SELECT * FROM kernel_configs ORDER BY harness ASC",
        )
        return [_row_to_record(row) for row in rows]

    async def get(self, harness: HarnessName) -> KernelConfigRecord | None:
        row = await self._db.fetch_one(
            "SELECT * FROM kernel_configs WHERE harness = ?",
            (harness.value,),
        )
        return _row_to_record(row) if row is not None else None

    async def upsert(
        self,
        harness: HarnessName,
        env_vars: str,
    ) -> KernelConfigRecord:
        now = utc_now()
        await self._db.execute(
            """
            INSERT INTO kernel_configs (harness, env_vars, updated_at)
            VALUES (?, ?, ?)
            ON CONFLICT(harness) DO UPDATE SET
                env_vars = excluded.env_vars,
                updated_at = excluded.updated_at
            """,
            (harness.value, env_vars, now),
        )
        return KernelConfigRecord(
            harness=harness,
            env_vars=env_vars,
            updated_at=now,
        )


def _row_to_record(row: object) -> KernelConfigRecord:
    mapping: dict[str, object] = dict(row)  # type: ignore[arg-type]
    return KernelConfigRecord(
        harness=HarnessName(str(mapping["harness"])),
        env_vars=str(mapping["env_vars"]),
        updated_at=str(mapping["updated_at"]),
    )
