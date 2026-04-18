"""SQLite connection management shared by all client_service stores."""

from __future__ import annotations

import asyncio
import logging
import sqlite3
from pathlib import Path
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Callable, Iterable

logger = logging.getLogger(__name__)


class Database:
    """Async wrapper around a single SQLite connection.

    All access is funneled through ``asyncio.to_thread`` and serialized by
    a per-instance lock so that ``check_same_thread=False`` is safe.
    """

    def __init__(self, path: str | Path) -> None:
        self._path = Path(path)
        self._lock = asyncio.Lock()
        self._conn: sqlite3.Connection | None = None

    @property
    def path(self) -> Path:
        return self._path

    async def connect(self) -> None:
        if self._conn is not None:
            return
        if str(self._path) != ":memory:":
            self._path.parent.mkdir(parents=True, exist_ok=True)
        conn = await asyncio.to_thread(self._open)
        self._conn = conn
        logger.info("opened sqlite database at %s", self._path)

    def _open(self) -> sqlite3.Connection:
        conn = sqlite3.connect(
            self._path,
            check_same_thread=False,
            isolation_level=None,
        )
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("PRAGMA foreign_keys=ON")
        conn.execute("PRAGMA synchronous=NORMAL")
        return conn

    async def close(self) -> None:
        async with self._lock:
            if self._conn is None:
                return
            conn = self._conn
            self._conn = None
            await asyncio.to_thread(conn.close)

    async def execute(self, sql: str, params: Iterable[Any] = ()) -> None:
        async with self._lock:
            conn = self._require_conn()
            await asyncio.to_thread(conn.execute, sql, tuple(params))

    async def executescript(self, script: str) -> None:
        async with self._lock:
            conn = self._require_conn()
            await asyncio.to_thread(conn.executescript, script)

    async def fetch_all(
        self,
        sql: str,
        params: Iterable[Any] = (),
    ) -> list[sqlite3.Row]:
        return await self._run_fetch(
            lambda conn: conn.execute(sql, tuple(params)).fetchall(),
        )

    async def fetch_one(
        self,
        sql: str,
        params: Iterable[Any] = (),
    ) -> sqlite3.Row | None:
        return await self._run_fetch(
            lambda conn: conn.execute(sql, tuple(params)).fetchone(),
        )

    async def _run_fetch[T](self, fn: Callable[[sqlite3.Connection], T]) -> T:
        async with self._lock:
            conn = self._require_conn()
            return await asyncio.to_thread(fn, conn)

    def _require_conn(self) -> sqlite3.Connection:
        if self._conn is None:
            msg = "database is not connected; call connect() first"
            raise RuntimeError(msg)
        return self._conn
