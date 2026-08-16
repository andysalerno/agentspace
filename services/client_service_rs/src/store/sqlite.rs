use std::{
    collections::BTreeSet,
    fmt::{self, Formatter},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, Error as RusqliteError, ErrorCode, OptionalExtension, Row, params};

use crate::{
    errors::{StoreError, ValidationError},
    models::{
        CliHarnessName, CliLaunchSnapshot, ClientType, MessageRecord, MessageRole, RuntimeStatus,
        SessionRecord, ToolCallRecord, WorkspaceMountRecord, WorkspaceRecord, WorkspaceStatus,
    },
};
use tracing::{debug, info, warn};

const WORKSPACES_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'ready',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
";

const SESSIONS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS client_sessions (
    session_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    agent_host_session_id TEXT NOT NULL,
    status TEXT NOT NULL,
    channel_name TEXT,
    client_type TEXT,
    interaction_mode TEXT NOT NULL DEFAULT 'chat',
    cli_harness TEXT,
    cli_connection_id TEXT,
    harness_session_id TEXT,
    runtime_generation INTEGER,
    runtime_status TEXT,
    workspace_volume_identity TEXT,
    telemetry_volume_identity TEXT,
    workspace_mounts TEXT,
    launch_snapshot TEXT,
    vscode_url TEXT,
    free_port_url TEXT,
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
";

impl From<RusqliteError> for StoreError {
    fn from(error: RusqliteError) -> Self {
        Self::Persistence {
            store: "sqlite",
            detail: error.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct SqliteDatabase {
    path: Arc<PathBuf>,
    connection: Arc<Mutex<Connection>>,
}

impl SqliteDatabase {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let in_memory = path == Path::new(":memory:");
        info!(in_memory, "opening sqlite store database");
        if path != Path::new(":memory:")
            && let Some(parent) = path.parent()
        {
            fs::create_dir_all(parent).map_err(|error| {
                warn!(in_memory, "failed to create sqlite database directory");
                StoreError::Persistence {
                    store: "sqlite",
                    detail: format!("failed to create database directory: {error}"),
                }
            })?;
        }
        let connection = Connection::open(&path).map_err(|error| {
            warn!(in_memory, "failed to open sqlite store database");
            StoreError::from(error)
        })?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;\nPRAGMA journal_mode=WAL;\nPRAGMA synchronous=NORMAL;",
            )
            .map_err(|error| {
                warn!(in_memory, "failed to configure sqlite store database");
                StoreError::from(error)
            })?;
        info!(in_memory, "opened sqlite store database");
        Ok(Self {
            path: Arc::new(path),
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with_connection<T>(
        &self,
        store: &'static str,
        action: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let guard = self.connection.lock().map_err(|_error| {
            warn!(store, "sqlite store lock poisoned");
            StoreError::LockPoisoned { store }
        })?;
        let result = action(&guard);
        drop(guard);
        trace_store_result(store, &result);
        result
    }

    fn with_mut_connection<T>(
        &self,
        store: &'static str,
        action: impl FnOnce(&mut Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut guard = self.connection.lock().map_err(|_error| {
            warn!(store, "sqlite store lock poisoned");
            StoreError::LockPoisoned { store }
        })?;
        let result = action(&mut guard);
        drop(guard);
        trace_store_result(store, &result);
        result
    }
}

impl fmt::Debug for SqliteDatabase {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteDatabase")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct SqliteStoreSet {
    pub(super) workspaces: SqliteWorkspaceStore,
    pub(super) sessions: SqliteSessionStore,
}

impl SqliteStoreSet {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        debug!("opening sqlite store set");
        let database = SqliteDatabase::open(path)?;
        initialize_schema(&database)?;
        info!("sqlite store set ready");
        Ok(Self {
            workspaces: SqliteWorkspaceStore::new(database.clone()),
            sessions: SqliteSessionStore::new(database),
        })
    }
}

#[derive(Clone, Debug)]
pub struct SqliteWorkspaceStore {
    database: SqliteDatabase,
}

impl SqliteWorkspaceStore {
    #[must_use]
    pub const fn new(database: SqliteDatabase) -> Self {
        Self { database }
    }

    pub fn list(&self) -> Result<Vec<WorkspaceRecord>, StoreError> {
        self.database.with_connection("workspaces", |connection| {
            let mut statement = connection
                .prepare("SELECT * FROM workspaces ORDER BY created_at ASC, workspace_id ASC")?;
            let rows = statement.query_and_then([], row_to_workspace)?;
            let records = rows.collect::<Result<Vec<_>, StoreError>>()?;
            debug!(
                store = "workspaces",
                count = records.len(),
                "listed workspaces"
            );
            Ok(records)
        })
    }

    pub fn get(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>, StoreError> {
        self.database.with_connection("workspaces", |connection| {
            let mut statement =
                connection.prepare("SELECT * FROM workspaces WHERE workspace_id = ?")?;
            let mut rows = statement.query_and_then(params![workspace_id], row_to_workspace)?;
            let record = rows.next().transpose()?;
            debug!(
                store = "workspaces",
                workspace_id,
                found = record.is_some(),
                "looked up workspace"
            );
            Ok(record)
        })
    }

    pub fn insert(&self, workspace: WorkspaceRecord) -> Result<(), StoreError> {
        self.database.with_connection("workspaces", |connection| {
            match insert_workspace(connection, &workspace) {
                Ok(()) => {
                    debug!(
                        store = "workspaces",
                        workspace_id = %workspace.workspace_id,
                        "inserted workspace"
                    );
                    Ok(())
                }
                Err(error) if is_constraint(&error) => {
                    debug!(
                        store = "workspaces",
                        workspace_id = %workspace.workspace_id,
                        "workspace insert hit existing record"
                    );
                    Err(StoreError::WorkspaceAlreadyExists {
                        workspace_id: workspace.workspace_id,
                    })
                }
                Err(error) => Err(error.into()),
            }
        })
    }

    pub fn update(&self, workspace: WorkspaceRecord) -> Result<(), StoreError> {
        let workspace_id = workspace.workspace_id.clone();
        self.database.with_connection("workspaces", |connection| {
            if !workspace_exists(connection, &workspace.workspace_id)? {
                debug!(
                    store = "workspaces",
                    workspace_id = %workspace.workspace_id,
                    "workspace update missed existing record"
                );
                return Err(StoreError::WorkspaceNotFound {
                    workspace_id: workspace.workspace_id,
                });
            }
            connection.execute(
                "
                    UPDATE workspaces
                       SET name = ?,
                           status = ?,
                           updated_at = ?
                      WHERE workspace_id = ?
                    ",
                params![
                    workspace.name,
                    workspace.status.as_str(),
                    workspace.updated_at,
                    workspace.workspace_id
                ],
            )?;
            debug!(
                store = "workspaces",
                workspace_id = %workspace_id,
                "updated workspace"
            );
            Ok(())
        })
    }

    pub fn delete(&self, workspace_id: &str) -> Result<bool, StoreError> {
        self.database.with_connection("workspaces", |connection| {
            let deleted = connection.execute(
                "DELETE FROM workspaces WHERE workspace_id = ?",
                params![workspace_id],
            )? > 0;
            debug!(
                store = "workspaces",
                workspace_id, deleted, "deleted workspace"
            );
            Ok(deleted)
        })
    }
}

#[derive(Clone, Debug)]
pub struct SqliteSessionStore {
    database: SqliteDatabase,
}

impl SqliteSessionStore {
    #[must_use]
    pub const fn new(database: SqliteDatabase) -> Self {
        Self { database }
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>, StoreError> {
        self.database.with_connection("sessions", |connection| {
            let mut statement = connection
                .prepare("SELECT * FROM client_sessions ORDER BY created_at ASC, session_id ASC")?;
            let rows = statement.query_and_then([], row_to_session_without_messages)?;
            let mut sessions = rows.collect::<Result<Vec<_>, StoreError>>()?;
            for session in &mut sessions {
                session.messages = messages_for_session(connection, &session.session_id)?;
            }
            let message_count = sessions
                .iter()
                .map(|session| session.messages.len())
                .sum::<usize>();
            let tool_call_count = sessions
                .iter()
                .flat_map(|session| &session.messages)
                .map(|message| message.tool_calls.len())
                .sum::<usize>();
            debug!(
                store = "sessions",
                count = sessions.len(),
                message_count,
                tool_call_count,
                "listed sessions"
            );
            Ok(sessions)
        })
    }

    pub fn get(&self, session_id: &str) -> Result<Option<SessionRecord>, StoreError> {
        self.database.with_connection("sessions", |connection| {
            let mut statement =
                connection.prepare("SELECT * FROM client_sessions WHERE session_id = ?")?;
            let mut rows =
                statement.query_and_then(params![session_id], row_to_session_without_messages)?;
            let mut session = rows.next().transpose()?;
            if let Some(session) = &mut session {
                session.messages = messages_for_session(connection, &session.session_id)?;
            }
            let message_count = session.as_ref().map_or(0, |session| session.messages.len());
            let tool_call_count = session.as_ref().map_or(0, |session| {
                session
                    .messages
                    .iter()
                    .map(|message| message.tool_calls.len())
                    .sum::<usize>()
            });
            debug!(
                store = "sessions",
                session_id,
                found = session.is_some(),
                message_count,
                tool_call_count,
                "looked up session"
            );
            Ok(session)
        })
    }

    pub fn insert(&self, session: SessionRecord) -> Result<(), StoreError> {
        self.database.with_connection("sessions", |connection| {
            match insert_session(connection, &session) {
                Ok(()) => {
                    debug!(
                        store = "sessions",
                        session_id = %session.session_id,
                        agent_id = %session.agent_id,
                        status = %session.status,
                        client_type = session.client_type.map(ClientType::as_str),
                        channel_present = session.channel_name.is_some(),
                        "inserted session"
                    );
                    Ok(())
                }
                Err(error) if is_constraint(&error) => {
                    debug!(
                        store = "sessions",
                        session_id = %session.session_id,
                        "session insert hit existing record"
                    );
                    Err(StoreError::SessionAlreadyExists {
                        session_id: session.session_id,
                    })
                }
                Err(error) => Err(error.into()),
            }
        })
    }

    pub fn update(&self, session: SessionRecord) -> Result<(), StoreError> {
        let session_id = session.session_id.clone();
        let agent_id = session.agent_id.clone();
        let status = session.status.clone();
        let client_type = session.client_type.map(ClientType::as_str);
        let channel_present = session.channel_name.is_some();
        self.database.with_connection("sessions", |connection| {
            if !session_exists(connection, &session.session_id)? {
                debug!(
                    store = "sessions",
                    session_id = %session.session_id,
                    "session update missed existing record"
                );
                return Err(StoreError::SessionNotFound {
                    session_id: session.session_id,
                });
            }
            connection.execute(
                "
                UPDATE client_sessions
                   SET agent_id = ?,
                       agent_host_session_id = ?,
                       status = ?,
                       channel_name = ?,
                       client_type = ?,
                       interaction_mode = ?,
                       cli_harness = ?,
                       cli_connection_id = ?,
                       harness_session_id = ?,
                       runtime_generation = ?,
                       runtime_status = ?,
                       workspace_volume_identity = ?,
                       telemetry_volume_identity = ?,
                       workspace_mounts = ?,
                       launch_snapshot = ?,
                       vscode_url = ?,
                       free_port_url = ?,
                       updated_at = ?
                 WHERE session_id = ?
                ",
                params![
                    session.agent_id,
                    session.agent_host_session_id,
                    session.status,
                    session.channel_name,
                    session.client_type.map(ClientType::as_str),
                    session.interaction_mode.as_str(),
                    session.cli_harness.map(CliHarnessName::as_str),
                    session.cli_connection_id,
                    session.harness_session_id,
                    optional_u64_to_i64(session.runtime_generation)?,
                    session.runtime_status.map(RuntimeStatus::as_str),
                    session.workspace_volume_identity,
                    session.telemetry_volume_identity,
                    serialize_workspace_mounts(&session.workspace_mounts)?,
                    serialize_launch_snapshot(session.launch_snapshot.as_ref())?,
                    session.vscode_url,
                    session.free_port_url,
                    session.updated_at,
                    session.session_id,
                ],
            )?;
            debug!(
                store = "sessions",
                session_id = %session_id,
                agent_id = %agent_id,
                status = %status,
                client_type,
                channel_present,
                "updated session"
            );
            Ok(())
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn upsert(&self, session: SessionRecord) -> Result<(), StoreError> {
        let session_id = session.session_id.clone();
        let agent_id = session.agent_id.clone();
        let status = session.status.clone();
        let client_type = session.client_type.map(ClientType::as_str);
        let channel_present = session.channel_name.is_some();
        self.database.with_connection("sessions", |connection| {
            connection.execute(
                "
                INSERT INTO client_sessions (
                    session_id, agent_id, agent_host_session_id, status,
                    channel_name, client_type, interaction_mode, cli_harness,
                    cli_connection_id, harness_session_id, runtime_generation,
                    runtime_status, workspace_volume_identity, telemetry_volume_identity,
                    workspace_mounts, launch_snapshot,
                    vscode_url, free_port_url, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(session_id) DO UPDATE SET
                    agent_id = excluded.agent_id,
                    agent_host_session_id = excluded.agent_host_session_id,
                    status = excluded.status,
                    channel_name = excluded.channel_name,
                    client_type = excluded.client_type,
                    interaction_mode = excluded.interaction_mode,
                    cli_harness = excluded.cli_harness,
                    cli_connection_id = excluded.cli_connection_id,
                    harness_session_id = excluded.harness_session_id,
                    runtime_generation = excluded.runtime_generation,
                    runtime_status = excluded.runtime_status,
                    workspace_volume_identity = excluded.workspace_volume_identity,
                    telemetry_volume_identity = excluded.telemetry_volume_identity,
                    workspace_mounts = excluded.workspace_mounts,
                    launch_snapshot = excluded.launch_snapshot,
                    vscode_url = excluded.vscode_url,
                    free_port_url = excluded.free_port_url,
                    updated_at = excluded.updated_at
                ",
                params![
                    session.session_id,
                    session.agent_id,
                    session.agent_host_session_id,
                    session.status,
                    session.channel_name,
                    session.client_type.map(ClientType::as_str),
                    session.interaction_mode.as_str(),
                    session.cli_harness.map(CliHarnessName::as_str),
                    session.cli_connection_id,
                    session.harness_session_id,
                    optional_u64_to_i64(session.runtime_generation)?,
                    session.runtime_status.map(RuntimeStatus::as_str),
                    session.workspace_volume_identity,
                    session.telemetry_volume_identity,
                    serialize_workspace_mounts(&session.workspace_mounts)?,
                    serialize_launch_snapshot(session.launch_snapshot.as_ref())?,
                    session.vscode_url,
                    session.free_port_url,
                    session.created_at,
                    session.updated_at,
                ],
            )?;
            debug!(
                store = "sessions",
                session_id = %session_id,
                agent_id = %agent_id,
                status = %status,
                client_type,
                channel_present,
                "upserted session"
            );
            Ok(())
        })
    }

    pub fn delete(&self, session_id: &str) -> Result<bool, StoreError> {
        self.database.with_connection("sessions", |connection| {
            let deleted = connection.execute(
                "DELETE FROM client_sessions WHERE session_id = ?",
                params![session_id],
            )? > 0;
            debug!(store = "sessions", session_id, deleted, "deleted session");
            Ok(deleted)
        })
    }

    pub fn append_message(&self, message: &MessageRecord) -> Result<(), StoreError> {
        self.upsert_message("append_message", message)
    }

    pub fn update_message(&self, message: &MessageRecord) -> Result<(), StoreError> {
        self.upsert_message("update_message", message)
    }

    pub fn clear_messages(&self, session_id: &str) -> Result<(), StoreError> {
        self.database.with_connection("sessions", |connection| {
            if !session_exists(connection, session_id)? {
                debug!(
                    store = "sessions",
                    session_id, "clear messages missed existing session"
                );
                return Err(StoreError::SessionNotFound {
                    session_id: session_id.to_owned(),
                });
            }
            let deleted_message_count = connection.execute(
                "DELETE FROM client_messages WHERE session_id = ?",
                params![session_id],
            )?;
            debug!(
                store = "sessions",
                session_id, deleted_message_count, "cleared session messages"
            );
            Ok(())
        })
    }

    fn upsert_message(
        &self,
        operation: &'static str,
        message: &MessageRecord,
    ) -> Result<(), StoreError> {
        let message_id = message.message_id.clone();
        let session_id = message.session_id.clone();
        let role = message.role.as_str();
        let reasoning_present = !message.reasoning.is_empty();
        let tool_call_count = message.tool_calls.len();
        self.database.with_mut_connection("sessions", |connection| {
            if !session_exists(connection, &message.session_id)? {
                debug!(
                    store = "sessions",
                    operation,
                    session_id = %message.session_id,
                    message_id = %message.message_id,
                    "message persistence missed existing session"
                );
                return Err(StoreError::SessionNotFound {
                    session_id: message.session_id.clone(),
                });
            }
            let transaction = connection.transaction()?;
            transaction.execute(
                "
                    INSERT INTO client_messages (
                        message_id, session_id, role, content, reasoning, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?)
                    ON CONFLICT(message_id) DO UPDATE SET
                        role = excluded.role,
                        content = excluded.content,
                        reasoning = excluded.reasoning
                    ",
                params![
                    message.message_id,
                    message.session_id,
                    message.role.as_str(),
                    message.content,
                    message.reasoning,
                    message.created_at,
                ],
            )?;
            let replaced_tool_call_count = transaction.execute(
                "DELETE FROM client_message_tool_calls WHERE message_id = ?",
                params![message.message_id],
            )?;
            for (index, tool_call) in message.tool_calls.iter().enumerate() {
                transaction.execute(
                    "
                        INSERT INTO client_message_tool_calls (
                            message_id, idx, tool, tool_call_id, status, kind,
                            input, output, content_offset
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                        ",
                    params![
                        message.message_id,
                        index_to_i64(index)?,
                        tool_call.tool,
                        tool_call.tool_call_id,
                        tool_call.status,
                        tool_call.kind,
                        tool_call.input,
                        tool_call.output,
                        optional_usize_to_i64(tool_call.content_offset)?,
                    ],
                )?;
            }
            transaction.commit()?;
            debug!(
                store = "sessions",
                operation,
                session_id = %session_id,
                message_id = %message_id,
                role,
                reasoning_present,
                tool_call_count,
                replaced_tool_call_count,
                "persisted session message"
            );
            Ok(())
        })
    }
}

fn initialize_schema(database: &SqliteDatabase) -> Result<(), StoreError> {
    database.with_connection("sqlite", |connection| {
        debug!("initializing sqlite store schema");
        for (schema, sql) in [
            ("workspaces", WORKSPACES_SCHEMA),
            ("sessions", SESSIONS_SCHEMA),
        ] {
            connection.execute_batch(sql)?;
            debug!(schema, "ensured sqlite store schema");
        }
        ensure_column(
            connection,
            "workspaces",
            "status",
            "status TEXT NOT NULL DEFAULT 'ready'",
        )?;
        for (column, definition) in [
            (
                "interaction_mode",
                "interaction_mode TEXT NOT NULL DEFAULT 'chat'",
            ),
            ("cli_harness", "cli_harness TEXT"),
            ("cli_connection_id", "cli_connection_id TEXT"),
            ("harness_session_id", "harness_session_id TEXT"),
            ("runtime_generation", "runtime_generation INTEGER"),
            ("runtime_status", "runtime_status TEXT"),
            (
                "workspace_volume_identity",
                "workspace_volume_identity TEXT",
            ),
            (
                "telemetry_volume_identity",
                "telemetry_volume_identity TEXT",
            ),
            ("workspace_mounts", "workspace_mounts TEXT"),
            ("launch_snapshot", "launch_snapshot TEXT"),
            ("vscode_url", "vscode_url TEXT"),
            ("free_port_url", "free_port_url TEXT"),
        ] {
            ensure_column(connection, "client_sessions", column, definition)?;
        }
        info!("initialized sqlite store schema");
        Ok(())
    })
}

fn ensure_column(
    connection: &Connection,
    table: &'static str,
    column: &'static str,
    definition: &'static str,
) -> Result<(), StoreError> {
    let columns = table_columns(connection, table)?;
    if columns.contains(column) {
        debug!(table, column, "sqlite store column already present");
    } else {
        debug!(table, column, "adding sqlite store column");
        connection.execute(&format!("ALTER TABLE {table} ADD COLUMN {definition}"), [])?;
        info!(table, column, "added sqlite store column");
    }
    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> Result<BTreeSet<String>, StoreError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>("name"))?;
    rows.collect::<Result<BTreeSet<_>, _>>().map_err(Into::into)
}

fn trace_store_result<T>(store: &'static str, result: &Result<T, StoreError>) {
    if let Err(error) = result {
        match error {
            StoreError::LockPoisoned { .. } => {
                warn!(
                    store,
                    error_kind = "lock_poisoned",
                    "sqlite store operation failed"
                );
            }
            StoreError::Persistence {
                store: error_store, ..
            } => {
                warn!(
                    store,
                    error_store,
                    error_kind = "persistence",
                    "sqlite store operation failed"
                );
            }
            _ => {}
        }
    }
}

fn row_to_workspace(row: &Row<'_>) -> Result<WorkspaceRecord, StoreError> {
    Ok(WorkspaceRecord {
        workspace_id: row.get("workspace_id")?,
        name: row.get("name")?,
        status: parse_workspace_status(row.get::<_, String>("status")?.as_str())?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_session_without_messages(row: &Row<'_>) -> Result<SessionRecord, StoreError> {
    let client_type_raw: Option<String> = row.get("client_type")?;
    let interaction_mode_raw: Option<String> = row.get("interaction_mode")?;
    let cli_harness_raw: Option<String> = row.get("cli_harness")?;
    let runtime_generation_raw: Option<i64> = row.get("runtime_generation")?;
    let runtime_status_raw: Option<String> = row.get("runtime_status")?;
    let workspace_mounts_raw: Option<String> = row.get("workspace_mounts")?;
    let launch_snapshot_raw: Option<String> = row.get("launch_snapshot")?;
    Ok(SessionRecord {
        session_id: row.get("session_id")?,
        agent_id: row.get("agent_id")?,
        agent_host_session_id: row.get("agent_host_session_id")?,
        status: row.get("status")?,
        channel_name: row.get("channel_name")?,
        client_type: client_type_raw
            .as_deref()
            .map(parse_client_type)
            .transpose()?,
        interaction_mode: interaction_mode_raw
            .as_deref()
            .unwrap_or("chat")
            .parse()
            .map_err(validation_error("sessions"))?,
        cli_harness: cli_harness_raw
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(validation_error("sessions"))?,
        cli_connection_id: row.get("cli_connection_id")?,
        harness_session_id: row.get("harness_session_id")?,
        runtime_generation: runtime_generation_raw.map(i64_to_u64).transpose()?,
        runtime_status: runtime_status_raw
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(validation_error("sessions"))?,
        workspace_volume_identity: row.get("workspace_volume_identity")?,
        telemetry_volume_identity: row.get("telemetry_volume_identity")?,
        workspace_mounts: deserialize_workspace_mounts(workspace_mounts_raw.as_deref())?,
        launch_snapshot: deserialize_launch_snapshot(launch_snapshot_raw.as_deref())?,
        vscode_url: row.get("vscode_url")?,
        free_port_url: row.get("free_port_url")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        messages: Vec::new(),
    })
}

fn row_to_message(row: &Row<'_>, connection: &Connection) -> Result<MessageRecord, StoreError> {
    let message_id: String = row.get("message_id")?;
    let role_raw: String = row.get("role")?;
    Ok(MessageRecord {
        message_id: message_id.clone(),
        session_id: row.get("session_id")?,
        role: parse_message_role(&role_raw)?,
        content: row.get("content")?,
        created_at: row.get("created_at")?,
        tool_calls: tool_calls_for_message(connection, &message_id)?,
        reasoning: row.get("reasoning")?,
    })
}

fn row_to_tool_call(row: &Row<'_>) -> Result<ToolCallRecord, StoreError> {
    let content_offset: Option<i64> = row.get("content_offset")?;
    Ok(ToolCallRecord {
        tool: row.get("tool")?,
        tool_call_id: row.get("tool_call_id")?,
        status: row.get("status")?,
        kind: row.get("kind")?,
        input: row.get("input")?,
        output: row.get("output")?,
        content_offset: content_offset.map(i64_to_usize).transpose()?,
    })
}

fn messages_for_session(
    connection: &Connection,
    session_id: &str,
) -> Result<Vec<MessageRecord>, StoreError> {
    let mut statement = connection.prepare(
        "
        SELECT * FROM client_messages
         WHERE session_id = ?
         ORDER BY rowid ASC
        ",
    )?;
    let rows =
        statement.query_and_then(params![session_id], |row| row_to_message(row, connection))?;
    let messages = rows.collect::<Result<Vec<_>, StoreError>>()?;
    let tool_call_count = messages
        .iter()
        .map(|message| message.tool_calls.len())
        .sum::<usize>();
    debug!(
        store = "sessions",
        session_id,
        message_count = messages.len(),
        tool_call_count,
        "loaded session messages"
    );
    Ok(messages)
}

fn tool_calls_for_message(
    connection: &Connection,
    message_id: &str,
) -> Result<Vec<ToolCallRecord>, StoreError> {
    let mut statement = connection.prepare(
        "
        SELECT * FROM client_message_tool_calls
         WHERE message_id = ?
         ORDER BY idx ASC
        ",
    )?;
    let rows = statement.query_and_then(params![message_id], row_to_tool_call)?;
    let tool_calls = rows.collect::<Result<Vec<_>, StoreError>>()?;
    debug!(
        store = "sessions",
        message_id,
        tool_call_count = tool_calls.len(),
        "loaded message tool calls"
    );
    Ok(tool_calls)
}

fn insert_workspace(
    connection: &Connection,
    workspace: &WorkspaceRecord,
) -> Result<(), RusqliteError> {
    connection.execute(
        "
        INSERT INTO workspaces (
            workspace_id, name, status, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?)
        ",
        params![
            workspace.workspace_id,
            workspace.name,
            workspace.status.as_str(),
            workspace.created_at,
            workspace.updated_at,
        ],
    )?;
    Ok(())
}

fn insert_session(connection: &Connection, session: &SessionRecord) -> Result<(), RusqliteError> {
    connection.execute(
        "
        INSERT INTO client_sessions (
            session_id, agent_id, agent_host_session_id, status,
            channel_name, client_type, interaction_mode, cli_harness,
            cli_connection_id, harness_session_id, runtime_generation,
            runtime_status, workspace_volume_identity, telemetry_volume_identity,
            workspace_mounts, launch_snapshot,
            vscode_url, free_port_url, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ",
        params![
            session.session_id,
            session.agent_id,
            session.agent_host_session_id,
            session.status,
            session.channel_name,
            session.client_type.map(ClientType::as_str),
            session.interaction_mode.as_str(),
            session.cli_harness.map(CliHarnessName::as_str),
            session.cli_connection_id,
            session.harness_session_id,
            optional_u64_to_i64(session.runtime_generation).map_err(store_error_to_sqlite)?,
            session.runtime_status.map(RuntimeStatus::as_str),
            session.workspace_volume_identity,
            session.telemetry_volume_identity,
            serialize_workspace_mounts(&session.workspace_mounts).map_err(store_error_to_sqlite)?,
            serialize_launch_snapshot(session.launch_snapshot.as_ref())
                .map_err(store_error_to_sqlite)?,
            session.vscode_url,
            session.free_port_url,
            session.created_at,
            session.updated_at,
        ],
    )?;
    Ok(())
}

fn workspace_exists(connection: &Connection, workspace_id: &str) -> Result<bool, StoreError> {
    exists(
        connection,
        "SELECT 1 FROM workspaces WHERE workspace_id = ?",
        workspace_id,
    )
}

fn session_exists(connection: &Connection, session_id: &str) -> Result<bool, StoreError> {
    exists(
        connection,
        "SELECT 1 FROM client_sessions WHERE session_id = ?",
        session_id,
    )
}

fn exists(connection: &Connection, sql: &str, id: &str) -> Result<bool, StoreError> {
    Ok(connection
        .query_row(sql, params![id], |_row| Ok(()))
        .optional()?
        .is_some())
}

fn parse_workspace_status(value: &str) -> Result<WorkspaceStatus, StoreError> {
    WorkspaceStatus::from_str(value).map_err(validation_error("workspaces"))
}

fn parse_client_type(value: &str) -> Result<ClientType, StoreError> {
    match value {
        "cli" => Ok(ClientType::Cli),
        "webui" => Ok(ClientType::Webui),
        _ => Err(StoreError::Persistence {
            store: "sessions",
            detail: format!("unsupported client_type {value:?}"),
        }),
    }
}

fn parse_message_role(value: &str) -> Result<MessageRole, StoreError> {
    match value {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        "system" => Ok(MessageRole::System),
        _ => Err(StoreError::Persistence {
            store: "sessions",
            detail: format!("unsupported message role {value:?}"),
        }),
    }
}

fn validation_error(store: &'static str) -> impl FnOnce(ValidationError) -> StoreError {
    move |error| StoreError::Persistence {
        store,
        detail: error.to_string(),
    }
}

fn index_to_i64(index: usize) -> Result<i64, StoreError> {
    i64::try_from(index).map_err(|error| StoreError::Persistence {
        store: "sessions",
        detail: format!("tool call index is too large: {error}"),
    })
}

fn optional_usize_to_i64(value: Option<usize>) -> Result<Option<i64>, StoreError> {
    value.map(index_to_i64).transpose()
}

fn i64_to_usize(value: i64) -> Result<usize, StoreError> {
    usize::try_from(value).map_err(|error| StoreError::Persistence {
        store: "sessions",
        detail: format!("content_offset is invalid: {error}"),
    })
}

fn optional_u64_to_i64(value: Option<u64>) -> Result<Option<i64>, StoreError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|error| StoreError::Persistence {
                store: "sessions",
                detail: format!("runtime_generation is too large: {error}"),
            })
        })
        .transpose()
}

fn i64_to_u64(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|error| StoreError::Persistence {
        store: "sessions",
        detail: format!("runtime_generation is invalid: {error}"),
    })
}

fn serialize_workspace_mounts(
    mounts: &[WorkspaceMountRecord],
) -> Result<Option<String>, StoreError> {
    if mounts.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(mounts)
        .map(Some)
        .map_err(|error| StoreError::Persistence {
            store: "sessions",
            detail: format!("failed to serialize workspace_mounts: {error}"),
        })
}

fn deserialize_workspace_mounts(
    raw: Option<&str>,
) -> Result<Vec<WorkspaceMountRecord>, StoreError> {
    raw.map_or_else(
        || Ok(Vec::new()),
        |raw| {
            serde_json::from_str(raw).map_err(|error| StoreError::Persistence {
                store: "sessions",
                detail: format!("failed to deserialize workspace_mounts: {error}"),
            })
        },
    )
}

fn serialize_launch_snapshot(
    snapshot: Option<&CliLaunchSnapshot>,
) -> Result<Option<String>, StoreError> {
    snapshot
        .map(|snapshot| {
            serde_json::to_string(snapshot).map_err(|error| StoreError::Persistence {
                store: "sessions",
                detail: format!("failed to serialize launch_snapshot: {error}"),
            })
        })
        .transpose()
}

fn deserialize_launch_snapshot(raw: Option<&str>) -> Result<Option<CliLaunchSnapshot>, StoreError> {
    raw.map(|raw| {
        serde_json::from_str(raw).map_err(|error| StoreError::Persistence {
            store: "sessions",
            detail: format!("failed to deserialize launch_snapshot: {error}"),
        })
    })
    .transpose()
}

fn store_error_to_sqlite(error: StoreError) -> RusqliteError {
    RusqliteError::ToSqlConversionFailure(Box::new(error))
}

fn is_constraint(error: &RusqliteError) -> bool {
    matches!(
        error,
        RusqliteError::SqliteFailure(failure, _message)
            if failure.code == ErrorCode::ConstraintViolation
    )
}
