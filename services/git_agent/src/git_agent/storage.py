from __future__ import annotations

import json
import sqlite3
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import TYPE_CHECKING, cast

if TYPE_CHECKING:
    from collections.abc import Sequence
    from pathlib import Path


@dataclass(frozen=True, kw_only=True)
class PatchRequestRow:
    request_id: str
    target_ref: str
    base_sha: str | None
    status: str
    accepted: bool
    raw_patch: str
    patch_hash: str
    commit_message: str
    author: object | None
    requester: object | None
    argument: object | None
    response_to_request_id: str | None
    reviewer_id: str | None
    reviewer_session_id: str | None
    reviewer_summary: str | None
    head_before: str | None
    commit_sha: str | None
    comments: list[object]
    created_at: str
    updated_at: str

    def to_dict(self, *, include_raw_patch: bool) -> dict[str, object]:
        result: dict[str, object] = {
            "id": self.request_id,
            "request_id": self.request_id,
            "target_ref": self.target_ref,
            "base_sha": self.base_sha,
            "status": self.status,
            "accepted": self.accepted,
            "patch_hash": self.patch_hash,
            "commit_message": self.commit_message,
            "author": self.author,
            "requester": self.requester,
            "argument": self.argument,
            "response_to_request_id": self.response_to_request_id,
            "reviewer_id": self.reviewer_id,
            "reviewer_session_id": self.reviewer_session_id,
            "reviewer_summary": self.reviewer_summary,
            "head_before": self.head_before,
            "commit_sha": self.commit_sha,
            "comments": self.comments,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        }
        if include_raw_patch:
            result["raw_patch"] = self.raw_patch
        return result


@dataclass(frozen=True, kw_only=True)
class PatchRequestCreate:
    request_id: str
    target_ref: str
    base_sha: str | None
    raw_patch: str
    patch_hash: str
    commit_message: str
    author: object | None
    requester: object | None
    argument: object | None
    response_to_request_id: str | None
    reviewer_id: str | None


@dataclass(frozen=True, kw_only=True)
class PatchRequestUpdate:
    status: str
    accepted: bool
    comments: Sequence[object]
    head_before: str | None = None
    commit_sha: str | None = None
    reviewer_session_id: str | None = None
    reviewer_summary: str | None = None


class PatchStore:
    def __init__(self, db_path: Path) -> None:
        self._db_path = db_path

    def initialize(self) -> None:
        self._db_path.parent.mkdir(parents=True, exist_ok=True)
        with self._connect() as conn:
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS patch_requests (
                    id TEXT PRIMARY KEY,
                    target_ref TEXT NOT NULL,
                    base_sha TEXT,
                    status TEXT NOT NULL,
                    accepted INTEGER NOT NULL,
                    raw_patch TEXT NOT NULL,
                    patch_hash TEXT NOT NULL,
                    commit_message TEXT NOT NULL,
                    author_json TEXT,
                    requester_json TEXT,
                    argument_json TEXT,
                    response_to_request_id TEXT,
                    reviewer_id TEXT,
                    reviewer_session_id TEXT,
                    reviewer_summary TEXT,
                    head_before TEXT,
                    commit_sha TEXT,
                    comments_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                )
                """,
            )
            if "response_to_request_id" not in _patch_request_columns(conn):
                conn.execute(
                    "ALTER TABLE patch_requests ADD COLUMN response_to_request_id TEXT",
                )

    def create(self, data: PatchRequestCreate) -> PatchRequestRow:
        now = _now_iso()
        with self._connect() as conn:
            conn.execute(
                """
                INSERT INTO patch_requests (
                    id, target_ref, base_sha, status, accepted, raw_patch,
                    patch_hash, commit_message, author_json, requester_json,
                    argument_json, response_to_request_id, reviewer_id, comments_json,
                    created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    data.request_id,
                    data.target_ref,
                    data.base_sha,
                    "received",
                    0,
                    data.raw_patch,
                    data.patch_hash,
                    data.commit_message,
                    _to_json(data.author),
                    _to_json(data.requester),
                    _to_json(data.argument),
                    data.response_to_request_id,
                    data.reviewer_id,
                    "[]",
                    now,
                    now,
                ),
            )
        row = self.get(data.request_id)
        if row is None:
            msg = "created patch request could not be loaded"
            raise RuntimeError(msg)
        return row

    def update(self, request_id: str, update: PatchRequestUpdate) -> PatchRequestRow:
        now = _now_iso()
        with self._connect() as conn:
            conn.execute(
                """
                UPDATE patch_requests
                SET status = ?, accepted = ?, comments_json = ?, head_before = ?,
                    commit_sha = ?, reviewer_session_id = ?, reviewer_summary = ?,
                    updated_at = ?
                WHERE id = ?
                """,
                (
                    update.status,
                    int(update.accepted),
                    _to_json(update.comments) or "[]",
                    update.head_before,
                    update.commit_sha,
                    update.reviewer_session_id,
                    update.reviewer_summary,
                    now,
                    request_id,
                ),
            )
        row = self.get(request_id)
        if row is None:
            msg = f"patch request {request_id} not found after update"
            raise RuntimeError(msg)
        return row

    def get(self, request_id: str) -> PatchRequestRow | None:
        with self._connect() as conn:
            row = conn.execute(
                "SELECT * FROM patch_requests WHERE id = ?",
                (request_id,),
            ).fetchone()
        if row is None:
            return None
        return _row_to_model(row)

    def list(self, *, limit: int = 100) -> list[PatchRequestRow]:
        with self._connect() as conn:
            rows = conn.execute(
                """
                SELECT * FROM patch_requests
                ORDER BY created_at DESC
                LIMIT ?
                """,
                (limit,),
            ).fetchall()
        return [_row_to_model(row) for row in rows]

    def _connect(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self._db_path)
        conn.row_factory = sqlite3.Row
        return conn


def _row_to_model(row: sqlite3.Row) -> PatchRequestRow:
    return PatchRequestRow(
        request_id=cast("str", row["id"]),
        target_ref=cast("str", row["target_ref"]),
        base_sha=cast("str | None", row["base_sha"]),
        status=cast("str", row["status"]),
        accepted=bool(row["accepted"]),
        raw_patch=cast("str", row["raw_patch"]),
        patch_hash=cast("str", row["patch_hash"]),
        commit_message=cast("str", row["commit_message"]),
        author=_from_json(cast("str | None", row["author_json"])),
        requester=_from_json(cast("str | None", row["requester_json"])),
        argument=_from_json(cast("str | None", row["argument_json"])),
        response_to_request_id=cast("str | None", row["response_to_request_id"]),
        reviewer_id=cast("str | None", row["reviewer_id"]),
        reviewer_session_id=cast("str | None", row["reviewer_session_id"]),
        reviewer_summary=cast("str | None", row["reviewer_summary"]),
        head_before=cast("str | None", row["head_before"]),
        commit_sha=cast("str | None", row["commit_sha"]),
        comments=_json_list(cast("str | None", row["comments_json"])),
        created_at=cast("str", row["created_at"]),
        updated_at=cast("str", row["updated_at"]),
    )


def _patch_request_columns(conn: sqlite3.Connection) -> set[str]:
    rows = conn.execute("PRAGMA table_info(patch_requests)").fetchall()
    return {cast("str", row["name"]) for row in rows}


def _to_json(value: object | None) -> str | None:
    if value is None:
        return None
    return json.dumps(value, sort_keys=True, separators=(",", ":"), default=str)


def _from_json(value: str | None) -> object | None:
    if value is None:
        return None
    return json.loads(value)


def _json_list(value: str | None) -> list[object]:
    parsed = _from_json(value)
    if isinstance(parsed, list):
        return cast("list[object]", parsed)
    return []


def _now_iso() -> str:
    return datetime.now(UTC).isoformat()
