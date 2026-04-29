"""Session and message persistence for Client Service."""

from __future__ import annotations

from typing import TYPE_CHECKING, Protocol, cast

from client_service.models import (
    ClientType,
    MessageRecord,
    MessageRole,
    SessionRecord,
    ToolCallRecord,
)

if TYPE_CHECKING:
    from client_service.storage.db import Database

SESSIONS_SCHEMA = """
CREATE TABLE IF NOT EXISTS client_sessions (
    session_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    agent_host_session_id TEXT NOT NULL,
    status TEXT NOT NULL,
    channel_name TEXT,
    client_type TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS client_messages (
    message_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    reasoning TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES client_sessions(session_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS client_message_tool_calls (
    message_id TEXT NOT NULL,
    idx INTEGER NOT NULL,
    tool TEXT NOT NULL,
    tool_call_id TEXT,
    status TEXT,
    kind TEXT,
    input TEXT,
    output TEXT,
    content_offset INTEGER,
    PRIMARY KEY(message_id, idx),
    FOREIGN KEY(message_id) REFERENCES client_messages(message_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_client_messages_session
    ON client_messages(session_id, created_at);
"""


class SessionStore(Protocol):
    async def list(self) -> list[SessionRecord]: ...
    async def get(self, session_id: str) -> SessionRecord | None: ...
    async def insert(self, session: SessionRecord) -> None: ...
    async def update(self, session: SessionRecord) -> None: ...
    async def delete(self, session_id: str) -> bool: ...
    async def append_message(self, message: MessageRecord) -> None: ...
    async def update_message(self, message: MessageRecord) -> None: ...
    async def clear_messages(self, session_id: str) -> None: ...


class InMemorySessionStore:
    def __init__(self) -> None:
        self._sessions: dict[str, SessionRecord] = {}

    async def list(self) -> list[SessionRecord]:
        return list(self._sessions.values())

    async def get(self, session_id: str) -> SessionRecord | None:
        return self._sessions.get(session_id)

    async def insert(self, session: SessionRecord) -> None:
        self._sessions[session.session_id] = session

    async def update(self, session: SessionRecord) -> None:
        self._sessions[session.session_id] = session

    async def delete(self, session_id: str) -> bool:
        return self._sessions.pop(session_id, None) is not None

    async def append_message(self, message: MessageRecord) -> None:
        session = self._sessions[message.session_id]
        session.messages.append(message)

    async def update_message(self, message: MessageRecord) -> None:
        session = self._sessions[message.session_id]
        for index, existing in enumerate(session.messages):
            if existing.message_id == message.message_id:
                session.messages[index] = message
                return
        session.messages.append(message)

    async def clear_messages(self, session_id: str) -> None:
        self._sessions[session_id].messages.clear()


class SqliteSessionStore:
    def __init__(self, database: Database) -> None:
        self._db = database

    async def initialize(self) -> None:
        await self._db.executescript(SESSIONS_SCHEMA)

    async def list(self) -> list[SessionRecord]:
        rows = await self._db.fetch_all(
            "SELECT * FROM client_sessions ORDER BY created_at ASC",
        )
        return [await self._row_to_session(row, include_messages=True) for row in rows]

    async def get(self, session_id: str) -> SessionRecord | None:
        row = await self._db.fetch_one(
            "SELECT * FROM client_sessions WHERE session_id = ?",
            (session_id,),
        )
        if row is None:
            return None
        return await self._row_to_session(row, include_messages=True)

    async def insert(self, session: SessionRecord) -> None:
        await self._db.execute(
            """
            INSERT INTO client_sessions (
                session_id, agent_id, agent_host_session_id, status,
                channel_name, client_type, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                session.session_id,
                session.agent_id,
                session.agent_host_session_id,
                session.status,
                session.channel_name,
                session.client_type.value if session.client_type is not None else None,
                session.created_at,
                session.updated_at,
            ),
        )

    async def update(self, session: SessionRecord) -> None:
        await self._db.execute(
            """
            UPDATE client_sessions
               SET agent_id = ?,
                   agent_host_session_id = ?,
                   status = ?,
                   channel_name = ?,
                   client_type = ?,
                   updated_at = ?
             WHERE session_id = ?
            """,
            (
                session.agent_id,
                session.agent_host_session_id,
                session.status,
                session.channel_name,
                session.client_type.value if session.client_type is not None else None,
                session.updated_at,
                session.session_id,
            ),
        )

    async def delete(self, session_id: str) -> bool:
        existing = await self.get(session_id)
        if existing is None:
            return False
        await self._db.execute(
            "DELETE FROM client_sessions WHERE session_id = ?",
            (session_id,),
        )
        return True

    async def append_message(self, message: MessageRecord) -> None:
        await self._upsert_message(message)

    async def update_message(self, message: MessageRecord) -> None:
        await self._upsert_message(message)

    async def clear_messages(self, session_id: str) -> None:
        await self._db.execute(
            "DELETE FROM client_messages WHERE session_id = ?",
            (session_id,),
        )

    async def _upsert_message(self, message: MessageRecord) -> None:
        await self._db.execute(
            """
            INSERT INTO client_messages (
                message_id, session_id, role, content, reasoning, created_at
            ) VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(message_id) DO UPDATE SET
                role = excluded.role,
                content = excluded.content,
                reasoning = excluded.reasoning
            """,
            (
                message.message_id,
                message.session_id,
                message.role.value,
                message.content,
                message.reasoning,
                message.created_at,
            ),
        )
        await self._db.execute(
            "DELETE FROM client_message_tool_calls WHERE message_id = ?",
            (message.message_id,),
        )
        for index, tool_call_record in enumerate(message.tool_calls):
            await self._db.execute(
                """
                INSERT INTO client_message_tool_calls (
                    message_id, idx, tool, tool_call_id, status, kind,
                    input, output, content_offset
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    message.message_id,
                    index,
                    tool_call_record.tool,
                    tool_call_record.tool_call_id,
                    tool_call_record.status,
                    tool_call_record.kind,
                    tool_call_record.input,
                    tool_call_record.output,
                    tool_call_record.content_offset,
                ),
            )

    async def _row_to_session(
        self,
        row: object,
        *,
        include_messages: bool,
    ) -> SessionRecord:
        mapping: dict[str, object] = dict(row)  # type: ignore[arg-type]
        client_type_raw = mapping["client_type"]
        messages = (
            await self._messages_for_session(str(mapping["session_id"]))
            if include_messages
            else []
        )
        return SessionRecord(
            session_id=str(mapping["session_id"]),
            agent_id=str(mapping["agent_id"]),
            agent_host_session_id=str(mapping["agent_host_session_id"]),
            status=str(mapping["status"]),
            channel_name=_optional_str(mapping["channel_name"]),
            client_type=(
                ClientType(str(client_type_raw))
                if client_type_raw is not None
                else None
            ),
            created_at=str(mapping["created_at"]),
            updated_at=str(mapping["updated_at"]),
            messages=messages,
        )

    async def _messages_for_session(self, session_id: str) -> list[MessageRecord]:
        rows = await self._db.fetch_all(
            """
            SELECT * FROM client_messages
             WHERE session_id = ?
             ORDER BY rowid ASC
            """,
            (session_id,),
        )
        return [await self._row_to_message(row) for row in rows]

    async def _row_to_message(self, row: object) -> MessageRecord:
        mapping: dict[str, object] = dict(row)  # type: ignore[arg-type]
        message_id = str(mapping["message_id"])
        tool_rows = await self._db.fetch_all(
            """
            SELECT * FROM client_message_tool_calls
             WHERE message_id = ?
             ORDER BY idx ASC
            """,
            (message_id,),
        )
        return MessageRecord(
            message_id=message_id,
            session_id=str(mapping["session_id"]),
            role=MessageRole(str(mapping["role"])),
            content=str(mapping["content"]),
            created_at=str(mapping["created_at"]),
            tool_calls=[_row_to_tool_call(tool_row) for tool_row in tool_rows],
            reasoning=str(mapping["reasoning"]),
        )


def _row_to_tool_call(row: object) -> ToolCallRecord:
    mapping: dict[str, object] = dict(row)  # type: ignore[arg-type]
    raw_offset = mapping["content_offset"]
    return ToolCallRecord(
        tool=str(mapping["tool"]),
        tool_call_id=_optional_str(mapping["tool_call_id"]),
        status=_optional_str(mapping["status"]),
        kind=_optional_str(mapping["kind"]),
        input=_optional_str(mapping["input"]),
        output=_optional_str(mapping["output"]),
        content_offset=(
            int(cast("int | str", raw_offset)) if raw_offset is not None else None
        ),
    )


def _optional_str(value: object) -> str | None:
    return value if isinstance(value, str) else None
