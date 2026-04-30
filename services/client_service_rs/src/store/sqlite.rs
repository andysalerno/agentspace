use std::{
    collections::{BTreeMap, BTreeSet},
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
        AgentRecord, ClientType, ConnectionApiFlavor, ConnectionRecord, GatewayRecord, GatewayType,
        HarnessName, KernelConfigRecord, MessageRecord, MessageRole, SessionRecord, ToolCallRecord,
        utc_now,
    },
};
use tracing::{debug, info, warn};

const AGENTS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS agents (
    agent_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    harness TEXT NOT NULL,
    system_prompt TEXT NOT NULL DEFAULT '',
    skills_json TEXT NOT NULL DEFAULT '[]',
    env_vars TEXT NOT NULL DEFAULT '',
    connection_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
";

const KERNEL_CONFIGS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS kernel_configs (
    harness TEXT PRIMARY KEY,
    env_vars TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);
";

const CONNECTIONS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS connections (
    connection_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    api_flavor TEXT NOT NULL DEFAULT 'chat_completions',
    api_key TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
";

const GATEWAYS_SCHEMA: &str = "
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
";

const SESSIONS_SCHEMA: &str = "
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
    pub(super) agents: SqliteAgentStore,
    pub(super) kernel_configs: SqliteKernelConfigStore,
    pub(super) connections: SqliteConnectionStore,
    pub(super) gateways: SqliteGatewayStore,
    pub(super) sessions: SqliteSessionStore,
}

impl SqliteStoreSet {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        debug!("opening sqlite store set");
        let database = SqliteDatabase::open(path)?;
        initialize_schema(&database)?;
        info!("sqlite store set ready");
        Ok(Self {
            agents: SqliteAgentStore::new(database.clone()),
            kernel_configs: SqliteKernelConfigStore::new(database.clone()),
            connections: SqliteConnectionStore::new(database.clone()),
            gateways: SqliteGatewayStore::new(database.clone()),
            sessions: SqliteSessionStore::new(database),
        })
    }
}

#[derive(Clone, Debug)]
pub struct SqliteAgentStore {
    database: SqliteDatabase,
}

impl SqliteAgentStore {
    #[must_use]
    pub const fn new(database: SqliteDatabase) -> Self {
        Self { database }
    }

    pub fn list(&self) -> Result<Vec<AgentRecord>, StoreError> {
        self.database.with_connection("agents", |connection| {
            let mut statement =
                connection.prepare("SELECT * FROM agents ORDER BY created_at ASC, agent_id ASC")?;
            let rows = statement.query_and_then([], row_to_agent)?;
            let records = rows.collect::<Result<Vec<_>, StoreError>>()?;
            debug!(store = "agents", count = records.len(), "listed agents");
            Ok(records)
        })
    }

    pub fn get(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        self.database.with_connection("agents", |connection| {
            let mut statement = connection.prepare("SELECT * FROM agents WHERE agent_id = ?")?;
            let mut rows = statement.query_and_then(params![agent_id], row_to_agent)?;
            let record = rows.next().transpose()?;
            debug!(
                store = "agents",
                agent_id,
                found = record.is_some(),
                "looked up agent"
            );
            Ok(record)
        })
    }

    pub fn insert(&self, agent: AgentRecord) -> Result<(), StoreError> {
        self.database.with_connection("agents", |connection| {
            match insert_agent(connection, &agent) {
                Ok(()) => {
                    debug!(
                        store = "agents",
                        agent_id = %agent.agent_id,
                        harness = agent.harness.as_str(),
                        skills_count = agent.skills.len(),
                        connection_id_present = agent.connection_id.is_some(),
                        "inserted agent"
                    );
                    Ok(())
                }
                Err(error) if is_constraint(&error) => {
                    debug!(
                        store = "agents",
                        agent_id = %agent.agent_id,
                        "agent insert hit existing record"
                    );
                    Err(StoreError::AgentAlreadyExists {
                        agent_id: agent.agent_id,
                    })
                }
                Err(error) => Err(error.into()),
            }
        })
    }

    pub fn update(&self, agent: AgentRecord) -> Result<(), StoreError> {
        let agent_id = agent.agent_id.clone();
        let harness = agent.harness.as_str();
        let skills_count = agent.skills.len();
        let connection_id_present = agent.connection_id.is_some();
        self.database.with_connection("agents", |connection| {
            if !agent_exists(connection, &agent.agent_id)? {
                debug!(
                    store = "agents",
                    agent_id = %agent.agent_id,
                    "agent update missed existing record"
                );
                return Err(StoreError::AgentNotFound {
                    agent_id: agent.agent_id,
                });
            }
            connection.execute(
                "
                UPDATE agents
                   SET name = ?,
                       harness = ?,
                       system_prompt = ?,
                       skills_json = ?,
                       env_vars = ?,
                       connection_id = ?,
                       updated_at = ?
                 WHERE agent_id = ?
                ",
                params![
                    agent.name,
                    agent.harness.as_str(),
                    agent.system_prompt,
                    skills_json(&agent.skills)?,
                    agent.env_vars,
                    agent.connection_id,
                    agent.updated_at,
                    agent.agent_id,
                ],
            )?;
            debug!(
                store = "agents",
                agent_id = %agent_id,
                harness,
                skills_count,
                connection_id_present,
                "updated agent"
            );
            Ok(())
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn upsert(&self, agent: AgentRecord) -> Result<(), StoreError> {
        let agent_id = agent.agent_id.clone();
        let harness = agent.harness.as_str();
        let skills_count = agent.skills.len();
        let connection_id_present = agent.connection_id.is_some();
        self.database.with_connection("agents", |connection| {
            connection.execute(
                "
                INSERT INTO agents (
                    agent_id, name, harness, system_prompt,
                    skills_json, env_vars, connection_id, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(agent_id) DO UPDATE SET
                    name = excluded.name,
                    harness = excluded.harness,
                    system_prompt = excluded.system_prompt,
                    skills_json = excluded.skills_json,
                    env_vars = excluded.env_vars,
                    connection_id = excluded.connection_id,
                    updated_at = excluded.updated_at
                ",
                params![
                    agent.agent_id,
                    agent.name,
                    agent.harness.as_str(),
                    agent.system_prompt,
                    skills_json(&agent.skills)?,
                    agent.env_vars,
                    agent.connection_id,
                    agent.created_at,
                    agent.updated_at,
                ],
            )?;
            debug!(
                store = "agents",
                agent_id = %agent_id,
                harness,
                skills_count,
                connection_id_present,
                "upserted agent"
            );
            Ok(())
        })
    }

    pub fn delete(&self, agent_id: &str) -> Result<bool, StoreError> {
        self.database.with_connection("agents", |connection| {
            let deleted =
                connection.execute("DELETE FROM agents WHERE agent_id = ?", params![agent_id])? > 0;
            debug!(store = "agents", agent_id, deleted, "deleted agent");
            Ok(deleted)
        })
    }
}

#[derive(Clone, Debug)]
pub struct SqliteKernelConfigStore {
    database: SqliteDatabase,
}

impl SqliteKernelConfigStore {
    #[must_use]
    pub const fn new(database: SqliteDatabase) -> Self {
        Self { database }
    }

    pub fn list(&self) -> Result<Vec<KernelConfigRecord>, StoreError> {
        self.database
            .with_connection("kernel_configs", |connection| {
                let mut statement =
                    connection.prepare("SELECT * FROM kernel_configs ORDER BY harness ASC")?;
                let rows = statement.query_and_then([], row_to_kernel_config)?;
                let records = rows.collect::<Result<Vec<_>, StoreError>>()?;
                debug!(
                    store = "kernel_configs",
                    count = records.len(),
                    "listed kernel configs"
                );
                Ok(records)
            })
    }

    pub fn get(&self, harness: HarnessName) -> Result<Option<KernelConfigRecord>, StoreError> {
        self.database
            .with_connection("kernel_configs", |connection| {
                let mut statement =
                    connection.prepare("SELECT * FROM kernel_configs WHERE harness = ?")?;
                let mut rows =
                    statement.query_and_then(params![harness.as_str()], row_to_kernel_config)?;
                let record = rows.next().transpose()?;
                debug!(
                    store = "kernel_configs",
                    harness = harness.as_str(),
                    found = record.is_some(),
                    "looked up kernel config"
                );
                Ok(record)
            })
    }

    pub fn upsert(
        &self,
        harness: HarnessName,
        env_vars: impl Into<String>,
    ) -> Result<KernelConfigRecord, StoreError> {
        let env_vars = env_vars.into();
        let record = KernelConfigRecord {
            harness,
            env_vars,
            updated_at: utc_now(),
        };
        self.database
            .with_connection("kernel_configs", |connection| {
                connection.execute(
                    "
                    INSERT INTO kernel_configs (harness, env_vars, updated_at)
                    VALUES (?, ?, ?)
                    ON CONFLICT(harness) DO UPDATE SET
                        env_vars = excluded.env_vars,
                        updated_at = excluded.updated_at
                    ",
                    params![record.harness.as_str(), record.env_vars, record.updated_at],
                )?;
                debug!(
                    store = "kernel_configs",
                    harness = record.harness.as_str(),
                    "upserted kernel config"
                );
                Ok(())
            })?;
        Ok(record)
    }

    pub fn delete(&self, harness: HarnessName) -> Result<bool, StoreError> {
        self.database
            .with_connection("kernel_configs", |connection| {
                let deleted = connection.execute(
                    "DELETE FROM kernel_configs WHERE harness = ?",
                    params![harness.as_str()],
                )? > 0;
                debug!(
                    store = "kernel_configs",
                    harness = harness.as_str(),
                    deleted,
                    "deleted kernel config"
                );
                Ok(deleted)
            })
    }
}

#[derive(Clone, Debug)]
pub struct SqliteConnectionStore {
    database: SqliteDatabase,
}

impl SqliteConnectionStore {
    #[must_use]
    pub const fn new(database: SqliteDatabase) -> Self {
        Self { database }
    }

    pub fn list(&self) -> Result<Vec<ConnectionRecord>, StoreError> {
        self.database.with_connection("connections", |connection| {
            let mut statement = connection
                .prepare("SELECT * FROM connections ORDER BY created_at ASC, connection_id ASC")?;
            let rows = statement.query_and_then([], row_to_connection)?;
            let records = rows.collect::<Result<Vec<_>, StoreError>>()?;
            debug!(
                store = "connections",
                count = records.len(),
                "listed connections"
            );
            Ok(records)
        })
    }

    pub fn get(&self, connection_id: &str) -> Result<Option<ConnectionRecord>, StoreError> {
        self.database.with_connection("connections", |connection| {
            let mut statement =
                connection.prepare("SELECT * FROM connections WHERE connection_id = ?")?;
            let mut rows = statement.query_and_then(params![connection_id], row_to_connection)?;
            let record = rows.next().transpose()?;
            debug!(
                store = "connections",
                connection_id,
                found = record.is_some(),
                "looked up connection"
            );
            Ok(record)
        })
    }

    pub fn insert(&self, connection_record: ConnectionRecord) -> Result<(), StoreError> {
        self.database.with_connection("connections", |connection| {
            match insert_connection(connection, &connection_record) {
                Ok(()) => {
                    debug!(
                        store = "connections",
                        connection_id = %connection_record.connection_id,
                        api_flavor = connection_record.api_flavor.as_str(),
                        "inserted connection"
                    );
                    Ok(())
                }
                Err(error) if is_constraint(&error) => {
                    debug!(
                        store = "connections",
                        connection_id = %connection_record.connection_id,
                        "connection insert hit existing record"
                    );
                    Err(StoreError::ConnectionAlreadyExists {
                        connection_id: connection_record.connection_id,
                    })
                }
                Err(error) => Err(error.into()),
            }
        })
    }

    pub fn update(&self, connection_record: ConnectionRecord) -> Result<(), StoreError> {
        let connection_id = connection_record.connection_id.clone();
        let api_flavor = connection_record.api_flavor.as_str();
        self.database.with_connection("connections", |connection| {
            if !connection_exists(connection, &connection_record.connection_id)? {
                debug!(
                    store = "connections",
                    connection_id = %connection_record.connection_id,
                    "connection update missed existing record"
                );
                return Err(StoreError::ConnectionNotFound {
                    connection_id: connection_record.connection_id,
                });
            }
            connection.execute(
                "
                UPDATE connections
                   SET name = ?,
                       url = ?,
                       api_flavor = ?,
                       api_key = ?,
                       updated_at = ?
                 WHERE connection_id = ?
                ",
                params![
                    connection_record.name,
                    connection_record.url,
                    connection_record.api_flavor.as_str(),
                    connection_record.api_key,
                    connection_record.updated_at,
                    connection_record.connection_id,
                ],
            )?;
            debug!(
                store = "connections",
                connection_id = %connection_id,
                api_flavor,
                "updated connection"
            );
            Ok(())
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn upsert(&self, connection_record: ConnectionRecord) -> Result<(), StoreError> {
        let connection_id = connection_record.connection_id.clone();
        let api_flavor = connection_record.api_flavor.as_str();
        self.database.with_connection("connections", |connection| {
            connection.execute(
                "
                INSERT INTO connections (
                    connection_id, name, url, api_flavor, api_key, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(connection_id) DO UPDATE SET
                    name = excluded.name,
                    url = excluded.url,
                    api_flavor = excluded.api_flavor,
                    api_key = excluded.api_key,
                    updated_at = excluded.updated_at
                ",
                params![
                    connection_record.connection_id,
                    connection_record.name,
                    connection_record.url,
                    connection_record.api_flavor.as_str(),
                    connection_record.api_key,
                    connection_record.created_at,
                    connection_record.updated_at,
                ],
            )?;
            debug!(
                store = "connections",
                connection_id = %connection_id,
                api_flavor,
                "upserted connection"
            );
            Ok(())
        })
    }

    pub fn delete(&self, connection_id: &str) -> Result<bool, StoreError> {
        self.database.with_connection("connections", |connection| {
            let deleted = connection.execute(
                "DELETE FROM connections WHERE connection_id = ?",
                params![connection_id],
            )? > 0;
            debug!(
                store = "connections",
                connection_id, deleted, "deleted connection"
            );
            Ok(deleted)
        })
    }
}

#[derive(Clone, Debug)]
pub struct SqliteGatewayStore {
    database: SqliteDatabase,
}

impl SqliteGatewayStore {
    #[must_use]
    pub const fn new(database: SqliteDatabase) -> Self {
        Self { database }
    }

    pub fn list(&self) -> Result<Vec<GatewayRecord>, StoreError> {
        self.database.with_connection("gateways", |connection| {
            let mut statement = connection
                .prepare("SELECT * FROM gateways ORDER BY created_at ASC, gateway_id ASC")?;
            let rows = statement.query_and_then([], row_to_gateway)?;
            let records = rows.collect::<Result<Vec<_>, StoreError>>()?;
            debug!(store = "gateways", count = records.len(), "listed gateways");
            Ok(records)
        })
    }

    pub fn get(&self, gateway_id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        self.database.with_connection("gateways", |connection| {
            let mut statement =
                connection.prepare("SELECT * FROM gateways WHERE gateway_id = ?")?;
            let mut rows = statement.query_and_then(params![gateway_id], row_to_gateway)?;
            let record = rows.next().transpose()?;
            debug!(
                store = "gateways",
                gateway_id,
                found = record.is_some(),
                "looked up gateway"
            );
            Ok(record)
        })
    }

    pub fn insert(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        self.database.with_connection("gateways", |connection| {
            match insert_gateway(connection, &gateway) {
                Ok(()) => {
                    debug!(
                        store = "gateways",
                        gateway_id = %gateway.gateway_id,
                        agent_id = %gateway.agent_id,
                        gateway_type = gateway.gateway_type.as_str(),
                        enabled = gateway.enabled,
                        status = %gateway.status,
                        "inserted gateway"
                    );
                    Ok(())
                }
                Err(error) if is_constraint(&error) => {
                    debug!(
                        store = "gateways",
                        gateway_id = %gateway.gateway_id,
                        "gateway insert hit existing record"
                    );
                    Err(StoreError::GatewayAlreadyExists {
                        gateway_id: gateway.gateway_id,
                    })
                }
                Err(error) => Err(error.into()),
            }
        })
    }

    pub fn update(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        let gateway_id = gateway.gateway_id.clone();
        let agent_id = gateway.agent_id.clone();
        let gateway_type = gateway.gateway_type.as_str();
        let enabled = gateway.enabled;
        let status = gateway.status.clone();
        self.database.with_connection("gateways", |connection| {
            if !gateway_exists(connection, &gateway.gateway_id)? {
                debug!(
                    store = "gateways",
                    gateway_id = %gateway.gateway_id,
                    "gateway update missed existing record"
                );
                return Err(StoreError::GatewayNotFound {
                    gateway_id: gateway.gateway_id,
                });
            }
            connection.execute(
                "
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
                ",
                params![
                    gateway.name,
                    gateway.gateway_type.as_str(),
                    gateway.agent_id,
                    enabled_int(gateway.enabled),
                    gateway.env_vars,
                    secrets_json(&gateway.secrets)?,
                    gateway.status,
                    gateway.last_error,
                    gateway.container_name,
                    gateway.updated_at,
                    gateway.gateway_id,
                ],
            )?;
            debug!(
                store = "gateways",
                gateway_id = %gateway_id,
                agent_id = %agent_id,
                gateway_type,
                enabled,
                status = %status,
                "updated gateway"
            );
            Ok(())
        })
    }

    #[allow(clippy::needless_pass_by_value)]
    pub fn upsert(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        let gateway_id = gateway.gateway_id.clone();
        let agent_id = gateway.agent_id.clone();
        let gateway_type = gateway.gateway_type.as_str();
        let enabled = gateway.enabled;
        let status = gateway.status.clone();
        self.database.with_connection("gateways", |connection| {
            connection.execute(
                "
                INSERT INTO gateways (
                    gateway_id, name, gateway_type, agent_id, enabled,
                    env_vars, secrets_json, status, last_error,
                    container_name, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(gateway_id) DO UPDATE SET
                    name = excluded.name,
                    gateway_type = excluded.gateway_type,
                    agent_id = excluded.agent_id,
                    enabled = excluded.enabled,
                    env_vars = excluded.env_vars,
                    secrets_json = excluded.secrets_json,
                    status = excluded.status,
                    last_error = excluded.last_error,
                    container_name = excluded.container_name,
                    updated_at = excluded.updated_at
                ",
                params![
                    gateway.gateway_id,
                    gateway.name,
                    gateway.gateway_type.as_str(),
                    gateway.agent_id,
                    enabled_int(gateway.enabled),
                    gateway.env_vars,
                    secrets_json(&gateway.secrets)?,
                    gateway.status,
                    gateway.last_error,
                    gateway.container_name,
                    gateway.created_at,
                    gateway.updated_at,
                ],
            )?;
            debug!(
                store = "gateways",
                gateway_id = %gateway_id,
                agent_id = %agent_id,
                gateway_type,
                enabled,
                status = %status,
                "upserted gateway"
            );
            Ok(())
        })
    }

    pub fn delete(&self, gateway_id: &str) -> Result<bool, StoreError> {
        self.database.with_connection("gateways", |connection| {
            let deleted = connection.execute(
                "DELETE FROM gateways WHERE gateway_id = ?",
                params![gateway_id],
            )? > 0;
            debug!(store = "gateways", gateway_id, deleted, "deleted gateway");
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
                       updated_at = ?
                 WHERE session_id = ?
                ",
                params![
                    session.agent_id,
                    session.agent_host_session_id,
                    session.status,
                    session.channel_name,
                    session.client_type.map(ClientType::as_str),
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
                    channel_name, client_type, created_at, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(session_id) DO UPDATE SET
                    agent_id = excluded.agent_id,
                    agent_host_session_id = excluded.agent_host_session_id,
                    status = excluded.status,
                    channel_name = excluded.channel_name,
                    client_type = excluded.client_type,
                    updated_at = excluded.updated_at
                ",
                params![
                    session.session_id,
                    session.agent_id,
                    session.agent_host_session_id,
                    session.status,
                    session.channel_name,
                    session.client_type.map(ClientType::as_str),
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
            ("agents", AGENTS_SCHEMA),
            ("kernel_configs", KERNEL_CONFIGS_SCHEMA),
            ("connections", CONNECTIONS_SCHEMA),
            ("gateways", GATEWAYS_SCHEMA),
            ("sessions", SESSIONS_SCHEMA),
        ] {
            connection.execute_batch(sql)?;
            debug!(schema, "ensured sqlite store schema");
        }
        ensure_column(connection, "agents", "connection_id", "connection_id TEXT")?;
        ensure_column(
            connection,
            "connections",
            "api_flavor",
            "api_flavor TEXT NOT NULL DEFAULT 'chat_completions'",
        )?;
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

fn row_to_agent(row: &Row<'_>) -> Result<AgentRecord, StoreError> {
    let skills_json: String = row.get("skills_json")?;
    let decoded: serde_json::Value =
        serde_json::from_str(&skills_json).map_err(json_error("agents"))?;
    let skills = decoded
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let harness_raw: String = row.get("harness")?;
    Ok(AgentRecord {
        agent_id: row.get("agent_id")?,
        name: row.get("name")?,
        harness: parse_harness(&harness_raw)?,
        system_prompt: row.get("system_prompt")?,
        skills,
        env_vars: row.get("env_vars")?,
        connection_id: row.get("connection_id")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_kernel_config(row: &Row<'_>) -> Result<KernelConfigRecord, StoreError> {
    let harness_raw: String = row.get("harness")?;
    Ok(KernelConfigRecord {
        harness: parse_harness(&harness_raw)?,
        env_vars: row.get("env_vars")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_connection(row: &Row<'_>) -> Result<ConnectionRecord, StoreError> {
    let api_flavor_raw: String = row.get("api_flavor")?;
    Ok(ConnectionRecord {
        connection_id: row.get("connection_id")?,
        name: row.get("name")?,
        url: row.get("url")?,
        api_flavor: ConnectionApiFlavor::from_str(&api_flavor_raw).unwrap_or_default(),
        api_key: row.get("api_key")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_gateway(row: &Row<'_>) -> Result<GatewayRecord, StoreError> {
    let gateway_type_raw: String = row.get("gateway_type")?;
    let secrets_json: String = row.get("secrets_json")?;
    let secrets = serde_json::from_str::<BTreeMap<String, String>>(&secrets_json)
        .map_err(json_error("gateways"))?;
    let enabled: i64 = row.get("enabled")?;
    Ok(GatewayRecord {
        gateway_id: row.get("gateway_id")?,
        name: row.get("name")?,
        gateway_type: parse_gateway_type(&gateway_type_raw)?,
        agent_id: row.get("agent_id")?,
        enabled: enabled != 0,
        env_vars: row.get("env_vars")?,
        secrets,
        status: row.get("status")?,
        last_error: row.get("last_error")?,
        container_name: row.get("container_name")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_session_without_messages(row: &Row<'_>) -> Result<SessionRecord, StoreError> {
    let client_type_raw: Option<String> = row.get("client_type")?;
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

fn insert_agent(connection: &Connection, agent: &AgentRecord) -> Result<(), RusqliteError> {
    connection.execute(
        "
        INSERT INTO agents (
            agent_id, name, harness, system_prompt,
            skills_json, env_vars, connection_id, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        ",
        params![
            agent.agent_id,
            agent.name,
            agent.harness.as_str(),
            agent.system_prompt,
            skills_json(&agent.skills)?,
            agent.env_vars,
            agent.connection_id,
            agent.created_at,
            agent.updated_at,
        ],
    )?;
    Ok(())
}

fn insert_connection(
    connection: &Connection,
    connection_record: &ConnectionRecord,
) -> Result<(), RusqliteError> {
    connection.execute(
        "
        INSERT INTO connections (
            connection_id, name, url, api_flavor, api_key, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?)
        ",
        params![
            connection_record.connection_id,
            connection_record.name,
            connection_record.url,
            connection_record.api_flavor.as_str(),
            connection_record.api_key,
            connection_record.created_at,
            connection_record.updated_at,
        ],
    )?;
    Ok(())
}

fn insert_gateway(connection: &Connection, gateway: &GatewayRecord) -> Result<(), RusqliteError> {
    connection.execute(
        "
        INSERT INTO gateways (
            gateway_id, name, gateway_type, agent_id, enabled,
            env_vars, secrets_json, status, last_error,
            container_name, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ",
        params![
            gateway.gateway_id,
            gateway.name,
            gateway.gateway_type.as_str(),
            gateway.agent_id,
            enabled_int(gateway.enabled),
            gateway.env_vars,
            secrets_json(&gateway.secrets)?,
            gateway.status,
            gateway.last_error,
            gateway.container_name,
            gateway.created_at,
            gateway.updated_at,
        ],
    )?;
    Ok(())
}

fn insert_session(connection: &Connection, session: &SessionRecord) -> Result<(), RusqliteError> {
    connection.execute(
        "
        INSERT INTO client_sessions (
            session_id, agent_id, agent_host_session_id, status,
            channel_name, client_type, created_at, updated_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        ",
        params![
            session.session_id,
            session.agent_id,
            session.agent_host_session_id,
            session.status,
            session.channel_name,
            session.client_type.map(ClientType::as_str),
            session.created_at,
            session.updated_at,
        ],
    )?;
    Ok(())
}

fn agent_exists(connection: &Connection, agent_id: &str) -> Result<bool, StoreError> {
    exists(
        connection,
        "SELECT 1 FROM agents WHERE agent_id = ?",
        agent_id,
    )
}

fn connection_exists(connection: &Connection, connection_id: &str) -> Result<bool, StoreError> {
    exists(
        connection,
        "SELECT 1 FROM connections WHERE connection_id = ?",
        connection_id,
    )
}

fn gateway_exists(connection: &Connection, gateway_id: &str) -> Result<bool, StoreError> {
    exists(
        connection,
        "SELECT 1 FROM gateways WHERE gateway_id = ?",
        gateway_id,
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

fn skills_json(skills: &[String]) -> Result<String, RusqliteError> {
    serde_json::to_string(skills).map_err(to_sql_conversion_failure)
}

fn secrets_json(secrets: &BTreeMap<String, String>) -> Result<String, RusqliteError> {
    serde_json::to_string(secrets).map_err(to_sql_conversion_failure)
}

fn to_sql_conversion_failure(error: serde_json::Error) -> RusqliteError {
    RusqliteError::ToSqlConversionFailure(Box::new(error))
}

fn json_error(store: &'static str) -> impl FnOnce(serde_json::Error) -> StoreError {
    move |error| StoreError::Persistence {
        store,
        detail: error.to_string(),
    }
}

fn parse_harness(value: &str) -> Result<HarnessName, StoreError> {
    HarnessName::from_str(value).map_err(validation_error("kernel_configs"))
}

fn parse_gateway_type(value: &str) -> Result<GatewayType, StoreError> {
    GatewayType::from_str(value).map_err(validation_error("gateways"))
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

const fn enabled_int(enabled: bool) -> i64 {
    if enabled { 1 } else { 0 }
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

fn is_constraint(error: &RusqliteError) -> bool {
    matches!(
        error,
        RusqliteError::SqliteFailure(failure, _message)
            if failure.code == ErrorCode::ConstraintViolation
    )
}
