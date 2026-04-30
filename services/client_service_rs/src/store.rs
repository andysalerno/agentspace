use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

mod sqlite;

use crate::{
    errors::StoreError,
    models::{
        AgentRecord, ConnectionRecord, GatewayRecord, HarnessName, KernelConfigRecord,
        MessageRecord, SessionRecord, utc_now,
    },
};

pub use sqlite::SqliteDatabase;

#[derive(Clone, Debug, Default)]
pub struct InMemoryAgentStore {
    agents: Arc<RwLock<BTreeMap<String, AgentRecord>>>,
}

impl InMemoryAgentStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Result<Vec<AgentRecord>, StoreError> {
        let mut records = with_read(&self.agents, "agents", |agents| {
            agents.values().cloned().collect::<Vec<_>>()
        })?;
        records.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.agent_id.cmp(&right.agent_id))
        });
        Ok(records)
    }

    pub fn get(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        with_read(&self.agents, "agents", |agents| {
            agents.get(agent_id).cloned()
        })
    }

    pub fn insert(&self, agent: AgentRecord) -> Result<(), StoreError> {
        with_write(&self.agents, "agents", |agents| {
            if agents.contains_key(&agent.agent_id) {
                return Err(StoreError::AgentAlreadyExists {
                    agent_id: agent.agent_id,
                });
            }
            agents.insert(agent.agent_id.clone(), agent);
            Ok(())
        })?
    }

    pub fn update(&self, agent: AgentRecord) -> Result<(), StoreError> {
        with_write(&self.agents, "agents", |agents| {
            if !agents.contains_key(&agent.agent_id) {
                return Err(StoreError::AgentNotFound {
                    agent_id: agent.agent_id,
                });
            }
            agents.insert(agent.agent_id.clone(), agent);
            Ok(())
        })?
    }

    pub fn upsert(&self, agent: AgentRecord) -> Result<(), StoreError> {
        with_write(&self.agents, "agents", |agents| {
            agents.insert(agent.agent_id.clone(), agent);
        })
    }

    pub fn delete(&self, agent_id: &str) -> Result<bool, StoreError> {
        with_write(&self.agents, "agents", |agents| {
            agents.remove(agent_id).is_some()
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryKernelConfigStore {
    configs: Arc<RwLock<BTreeMap<HarnessName, KernelConfigRecord>>>,
}

impl InMemoryKernelConfigStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Result<Vec<KernelConfigRecord>, StoreError> {
        let mut records = with_read(&self.configs, "kernel_configs", |configs| {
            configs.values().cloned().collect::<Vec<_>>()
        })?;
        records.sort_by(|left, right| left.harness.as_str().cmp(right.harness.as_str()));
        Ok(records)
    }

    pub fn get(&self, harness: HarnessName) -> Result<Option<KernelConfigRecord>, StoreError> {
        with_read(&self.configs, "kernel_configs", |configs| {
            configs.get(&harness).cloned()
        })
    }

    pub fn upsert(
        &self,
        harness: HarnessName,
        env_vars: impl Into<String>,
    ) -> Result<KernelConfigRecord, StoreError> {
        let record = KernelConfigRecord {
            harness,
            env_vars: env_vars.into(),
            updated_at: utc_now(),
        };
        with_write(&self.configs, "kernel_configs", |configs| {
            configs.insert(harness, record.clone());
        })?;
        Ok(record)
    }

    pub fn delete(&self, harness: HarnessName) -> Result<bool, StoreError> {
        with_write(&self.configs, "kernel_configs", |configs| {
            configs.remove(&harness).is_some()
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryConnectionStore {
    connections: Arc<RwLock<BTreeMap<String, ConnectionRecord>>>,
}

impl InMemoryConnectionStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Result<Vec<ConnectionRecord>, StoreError> {
        let mut records = with_read(&self.connections, "connections", |connections| {
            connections.values().cloned().collect::<Vec<_>>()
        })?;
        records.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.connection_id.cmp(&right.connection_id))
        });
        Ok(records)
    }

    pub fn get(&self, connection_id: &str) -> Result<Option<ConnectionRecord>, StoreError> {
        with_read(&self.connections, "connections", |connections| {
            connections.get(connection_id).cloned()
        })
    }

    pub fn insert(&self, connection: ConnectionRecord) -> Result<(), StoreError> {
        with_write(&self.connections, "connections", |connections| {
            if connections.contains_key(&connection.connection_id) {
                return Err(StoreError::ConnectionAlreadyExists {
                    connection_id: connection.connection_id,
                });
            }
            connections.insert(connection.connection_id.clone(), connection);
            Ok(())
        })?
    }

    pub fn update(&self, connection: ConnectionRecord) -> Result<(), StoreError> {
        with_write(&self.connections, "connections", |connections| {
            if !connections.contains_key(&connection.connection_id) {
                return Err(StoreError::ConnectionNotFound {
                    connection_id: connection.connection_id,
                });
            }
            connections.insert(connection.connection_id.clone(), connection);
            Ok(())
        })?
    }

    pub fn upsert(&self, connection: ConnectionRecord) -> Result<(), StoreError> {
        with_write(&self.connections, "connections", |connections| {
            connections.insert(connection.connection_id.clone(), connection);
        })
    }

    pub fn delete(&self, connection_id: &str) -> Result<bool, StoreError> {
        with_write(&self.connections, "connections", |connections| {
            connections.remove(connection_id).is_some()
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryGatewayStore {
    gateways: Arc<RwLock<BTreeMap<String, GatewayRecord>>>,
}

impl InMemoryGatewayStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Result<Vec<GatewayRecord>, StoreError> {
        let mut records = with_read(&self.gateways, "gateways", |gateways| {
            gateways.values().cloned().collect::<Vec<_>>()
        })?;
        records.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.gateway_id.cmp(&right.gateway_id))
        });
        Ok(records)
    }

    pub fn get(&self, gateway_id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        with_read(&self.gateways, "gateways", |gateways| {
            gateways.get(gateway_id).cloned()
        })
    }

    pub fn insert(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        with_write(&self.gateways, "gateways", |gateways| {
            if gateways.contains_key(&gateway.gateway_id) {
                return Err(StoreError::GatewayAlreadyExists {
                    gateway_id: gateway.gateway_id,
                });
            }
            gateways.insert(gateway.gateway_id.clone(), gateway);
            Ok(())
        })?
    }

    pub fn update(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        with_write(&self.gateways, "gateways", |gateways| {
            if !gateways.contains_key(&gateway.gateway_id) {
                return Err(StoreError::GatewayNotFound {
                    gateway_id: gateway.gateway_id,
                });
            }
            gateways.insert(gateway.gateway_id.clone(), gateway);
            Ok(())
        })?
    }

    pub fn upsert(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        with_write(&self.gateways, "gateways", |gateways| {
            gateways.insert(gateway.gateway_id.clone(), gateway);
        })
    }

    pub fn delete(&self, gateway_id: &str) -> Result<bool, StoreError> {
        with_write(&self.gateways, "gateways", |gateways| {
            gateways.remove(gateway_id).is_some()
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemorySessionStore {
    sessions: Arc<RwLock<BTreeMap<String, SessionRecord>>>,
}

impl InMemorySessionStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let mut records = with_read(&self.sessions, "sessions", |sessions| {
            sessions.values().cloned().collect::<Vec<_>>()
        })?;
        records.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        Ok(records)
    }

    pub fn get(&self, session_id: &str) -> Result<Option<SessionRecord>, StoreError> {
        with_read(&self.sessions, "sessions", |sessions| {
            sessions.get(session_id).cloned()
        })
    }

    pub fn insert(&self, session: SessionRecord) -> Result<(), StoreError> {
        with_write(&self.sessions, "sessions", |sessions| {
            if sessions.contains_key(&session.session_id) {
                return Err(StoreError::SessionAlreadyExists {
                    session_id: session.session_id,
                });
            }
            sessions.insert(session.session_id.clone(), session);
            Ok(())
        })?
    }

    pub fn update(&self, session: SessionRecord) -> Result<(), StoreError> {
        with_write(&self.sessions, "sessions", |sessions| {
            if !sessions.contains_key(&session.session_id) {
                return Err(StoreError::SessionNotFound {
                    session_id: session.session_id,
                });
            }
            sessions.insert(session.session_id.clone(), session);
            Ok(())
        })?
    }

    pub fn upsert(&self, session: SessionRecord) -> Result<(), StoreError> {
        with_write(&self.sessions, "sessions", |sessions| {
            sessions.insert(session.session_id.clone(), session);
        })
    }

    pub fn delete(&self, session_id: &str) -> Result<bool, StoreError> {
        with_write(&self.sessions, "sessions", |sessions| {
            sessions.remove(session_id).is_some()
        })
    }

    pub fn append_message(&self, message: MessageRecord) -> Result<(), StoreError> {
        with_write(&self.sessions, "sessions", |sessions| {
            let session = sessions.get_mut(&message.session_id).ok_or_else(|| {
                StoreError::SessionNotFound {
                    session_id: message.session_id.clone(),
                }
            })?;
            session.messages.push(message);
            Ok(())
        })?
    }

    pub fn update_message(&self, message: MessageRecord) -> Result<(), StoreError> {
        with_write(&self.sessions, "sessions", |sessions| {
            let session = sessions.get_mut(&message.session_id).ok_or_else(|| {
                StoreError::SessionNotFound {
                    session_id: message.session_id.clone(),
                }
            })?;
            if let Some(existing) = session
                .messages
                .iter_mut()
                .find(|existing| existing.message_id == message.message_id)
            {
                *existing = message;
            } else {
                session.messages.push(message);
            }
            Ok(())
        })?
    }

    pub fn clear_messages(&self, session_id: &str) -> Result<(), StoreError> {
        with_write(&self.sessions, "sessions", |sessions| {
            let session =
                sessions
                    .get_mut(session_id)
                    .ok_or_else(|| StoreError::SessionNotFound {
                        session_id: session_id.to_owned(),
                    })?;
            session.messages.clear();
            Ok(())
        })?
    }
}

#[derive(Clone, Debug)]
pub struct StoreSet {
    pub(crate) agents: AgentStore,
    pub(crate) kernel_configs: KernelConfigStore,
    pub(crate) connections: ConnectionStore,
    pub(crate) gateways: GatewayStore,
    pub(crate) sessions: SessionStore,
}

impl StoreSet {
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            agents: AgentStore::in_memory(),
            kernel_configs: KernelConfigStore::in_memory(),
            connections: ConnectionStore::in_memory(),
            gateways: GatewayStore::in_memory(),
            sessions: SessionStore::in_memory(),
        }
    }

    pub fn sqlite(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let stores = sqlite::SqliteStoreSet::open(path)?;
        Ok(Self {
            agents: AgentStore::Sqlite(stores.agents),
            kernel_configs: KernelConfigStore::Sqlite(stores.kernel_configs),
            connections: ConnectionStore::Sqlite(stores.connections),
            gateways: GatewayStore::Sqlite(stores.gateways),
            sessions: SessionStore::Sqlite(stores.sessions),
        })
    }
}

impl Default for StoreSet {
    fn default() -> Self {
        Self::in_memory()
    }
}

#[derive(Clone, Debug)]
pub enum AgentStore {
    InMemory(InMemoryAgentStore),
    Sqlite(sqlite::SqliteAgentStore),
}

impl AgentStore {
    #[must_use]
    pub fn in_memory() -> Self {
        Self::InMemory(InMemoryAgentStore::new())
    }

    pub fn list(&self) -> Result<Vec<AgentRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.list(),
            Self::Sqlite(store) => store.list(),
        }
    }

    pub fn get(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.get(agent_id),
            Self::Sqlite(store) => store.get(agent_id),
        }
    }

    pub fn insert(&self, agent: AgentRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.insert(agent),
            Self::Sqlite(store) => store.insert(agent),
        }
    }

    pub fn update(&self, agent: AgentRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.update(agent),
            Self::Sqlite(store) => store.update(agent),
        }
    }

    pub fn upsert(&self, agent: AgentRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.upsert(agent),
            Self::Sqlite(store) => store.upsert(agent),
        }
    }

    pub fn delete(&self, agent_id: &str) -> Result<bool, StoreError> {
        match self {
            Self::InMemory(store) => store.delete(agent_id),
            Self::Sqlite(store) => store.delete(agent_id),
        }
    }
}

#[derive(Clone, Debug)]
pub enum KernelConfigStore {
    InMemory(InMemoryKernelConfigStore),
    Sqlite(sqlite::SqliteKernelConfigStore),
}

impl KernelConfigStore {
    #[must_use]
    pub fn in_memory() -> Self {
        Self::InMemory(InMemoryKernelConfigStore::new())
    }

    pub fn list(&self) -> Result<Vec<KernelConfigRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.list(),
            Self::Sqlite(store) => store.list(),
        }
    }

    pub fn get(&self, harness: HarnessName) -> Result<Option<KernelConfigRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.get(harness),
            Self::Sqlite(store) => store.get(harness),
        }
    }

    pub fn upsert(
        &self,
        harness: HarnessName,
        env_vars: impl Into<String>,
    ) -> Result<KernelConfigRecord, StoreError> {
        let env_vars = env_vars.into();
        match self {
            Self::InMemory(store) => store.upsert(harness, env_vars),
            Self::Sqlite(store) => store.upsert(harness, env_vars),
        }
    }

    pub fn delete(&self, harness: HarnessName) -> Result<bool, StoreError> {
        match self {
            Self::InMemory(store) => store.delete(harness),
            Self::Sqlite(store) => store.delete(harness),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ConnectionStore {
    InMemory(InMemoryConnectionStore),
    Sqlite(sqlite::SqliteConnectionStore),
}

impl ConnectionStore {
    #[must_use]
    pub fn in_memory() -> Self {
        Self::InMemory(InMemoryConnectionStore::new())
    }

    pub fn list(&self) -> Result<Vec<ConnectionRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.list(),
            Self::Sqlite(store) => store.list(),
        }
    }

    pub fn get(&self, connection_id: &str) -> Result<Option<ConnectionRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.get(connection_id),
            Self::Sqlite(store) => store.get(connection_id),
        }
    }

    pub fn insert(&self, connection: ConnectionRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.insert(connection),
            Self::Sqlite(store) => store.insert(connection),
        }
    }

    pub fn update(&self, connection: ConnectionRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.update(connection),
            Self::Sqlite(store) => store.update(connection),
        }
    }

    pub fn upsert(&self, connection: ConnectionRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.upsert(connection),
            Self::Sqlite(store) => store.upsert(connection),
        }
    }

    pub fn delete(&self, connection_id: &str) -> Result<bool, StoreError> {
        match self {
            Self::InMemory(store) => store.delete(connection_id),
            Self::Sqlite(store) => store.delete(connection_id),
        }
    }
}

#[derive(Clone, Debug)]
pub enum GatewayStore {
    InMemory(InMemoryGatewayStore),
    Sqlite(sqlite::SqliteGatewayStore),
}

impl GatewayStore {
    #[must_use]
    pub fn in_memory() -> Self {
        Self::InMemory(InMemoryGatewayStore::new())
    }

    pub fn list(&self) -> Result<Vec<GatewayRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.list(),
            Self::Sqlite(store) => store.list(),
        }
    }

    pub fn get(&self, gateway_id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.get(gateway_id),
            Self::Sqlite(store) => store.get(gateway_id),
        }
    }

    pub fn insert(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.insert(gateway),
            Self::Sqlite(store) => store.insert(gateway),
        }
    }

    pub fn update(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.update(gateway),
            Self::Sqlite(store) => store.update(gateway),
        }
    }

    pub fn upsert(&self, gateway: GatewayRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.upsert(gateway),
            Self::Sqlite(store) => store.upsert(gateway),
        }
    }

    pub fn delete(&self, gateway_id: &str) -> Result<bool, StoreError> {
        match self {
            Self::InMemory(store) => store.delete(gateway_id),
            Self::Sqlite(store) => store.delete(gateway_id),
        }
    }
}

#[derive(Clone, Debug)]
pub enum SessionStore {
    InMemory(InMemorySessionStore),
    Sqlite(sqlite::SqliteSessionStore),
}

impl SessionStore {
    #[must_use]
    pub fn in_memory() -> Self {
        Self::InMemory(InMemorySessionStore::new())
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.list(),
            Self::Sqlite(store) => store.list(),
        }
    }

    pub fn get(&self, session_id: &str) -> Result<Option<SessionRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.get(session_id),
            Self::Sqlite(store) => store.get(session_id),
        }
    }

    pub fn insert(&self, session: SessionRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.insert(session),
            Self::Sqlite(store) => store.insert(session),
        }
    }

    pub fn update(&self, session: SessionRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.update(session),
            Self::Sqlite(store) => store.update(session),
        }
    }

    pub fn upsert(&self, session: SessionRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.upsert(session),
            Self::Sqlite(store) => store.upsert(session),
        }
    }

    pub fn delete(&self, session_id: &str) -> Result<bool, StoreError> {
        match self {
            Self::InMemory(store) => store.delete(session_id),
            Self::Sqlite(store) => store.delete(session_id),
        }
    }

    pub fn append_message(&self, message: MessageRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.append_message(message),
            Self::Sqlite(store) => store.append_message(message),
        }
    }

    pub fn update_message(&self, message: MessageRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.update_message(message),
            Self::Sqlite(store) => store.update_message(message),
        }
    }

    pub fn clear_messages(&self, session_id: &str) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.clear_messages(session_id),
            Self::Sqlite(store) => store.clear_messages(session_id),
        }
    }
}

fn read_lock<'a, T>(
    lock: &'a RwLock<T>,
    store: &'static str,
) -> Result<RwLockReadGuard<'a, T>, StoreError> {
    lock.read()
        .map_err(|_error| StoreError::LockPoisoned { store })
}

fn write_lock<'a, T>(
    lock: &'a RwLock<T>,
    store: &'static str,
) -> Result<RwLockWriteGuard<'a, T>, StoreError> {
    lock.write()
        .map_err(|_error| StoreError::LockPoisoned { store })
}

fn with_read<T, R>(
    lock: &RwLock<T>,
    store: &'static str,
    action: impl FnOnce(&T) -> R,
) -> Result<R, StoreError> {
    let guard = read_lock(lock, store)?;
    let result = action(&guard);
    drop(guard);
    Ok(result)
}

fn with_write<T, R>(
    lock: &RwLock<T>,
    store: &'static str,
    action: impl FnOnce(&mut T) -> R,
) -> Result<R, StoreError> {
    let mut guard = write_lock(lock, store)?;
    let result = action(&mut guard);
    drop(guard);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs,
        path::{Path, PathBuf},
    };

    use crate::{
        errors::StoreError,
        models::{
            AgentRecord, ClientType, ConnectionApiFlavor, ConnectionRecord, GatewayRecord,
            GatewayType, HarnessName, MessageRecord, MessageRole, SessionRecord, ToolCallRecord,
        },
    };

    use super::{
        InMemoryAgentStore, InMemoryConnectionStore, InMemoryGatewayStore,
        InMemoryKernelConfigStore, InMemorySessionStore, StoreSet,
    };

    fn agent(agent_id: &str, created_at: &str) -> AgentRecord {
        let mut record = AgentRecord::new(agent_id, agent_id, HarnessName::Acp, "prompt");
        record.created_at = created_at.to_owned();
        record.updated_at = created_at.to_owned();
        record
    }

    fn connection(connection_id: &str, created_at: &str) -> ConnectionRecord {
        let mut record = ConnectionRecord::new(connection_id, connection_id, "http://example.test");
        record.created_at = created_at.to_owned();
        record.updated_at = created_at.to_owned();
        record
    }

    fn gateway(gateway_id: &str, created_at: &str) -> GatewayRecord {
        let mut record =
            GatewayRecord::new(gateway_id, gateway_id, GatewayType::Echo, "agent", true);
        record.created_at = created_at.to_owned();
        record.updated_at = created_at.to_owned();
        record
    }

    fn session(session_id: &str, created_at: &str) -> SessionRecord {
        let mut record =
            SessionRecord::new(session_id, "agent", "host-session", "running", None, None);
        record.created_at = created_at.to_owned();
        record.updated_at = created_at.to_owned();
        record
    }

    fn sqlite_test_path() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sqlite-tests");
        fs::create_dir_all(&directory)?;
        Ok(directory.join(format!("{}.db", uuid::Uuid::now_v7().simple())))
    }

    fn cleanup_sqlite_path(path: &Path) {
        let raw = path.to_string_lossy();
        for candidate in [
            path.to_path_buf(),
            PathBuf::from(format!("{raw}-wal")),
            PathBuf::from(format!("{raw}-shm")),
        ] {
            let _ignored = fs::remove_file(candidate);
        }
    }

    #[test]
    fn agent_store_sorts_and_reports_duplicate_and_missing()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let store = InMemoryAgentStore::new();
        store.insert(agent("second", "2024-01-02"))?;
        store.insert(agent("first", "2024-01-01"))?;

        let ids = store
            .list()?
            .into_iter()
            .map(|record| record.agent_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["first", "second"]);

        let duplicate = store.insert(agent("first", "2024-01-03"));
        assert!(matches!(
            duplicate,
            Err(StoreError::AgentAlreadyExists { agent_id }) if agent_id == "first"
        ));

        let missing = store.update(agent("missing", "2024-01-03"));
        assert!(matches!(
            missing,
            Err(StoreError::AgentNotFound { agent_id }) if agent_id == "missing"
        ));
        assert!(store.delete("first")?);
        assert!(!store.delete("first")?);
        Ok(())
    }

    #[test]
    fn connection_store_duplicate_missing_and_upsert() -> Result<(), Box<dyn Error + Send + Sync>> {
        let store = InMemoryConnectionStore::new();
        store.insert(connection("conn", "2024-01-01"))?;
        assert!(matches!(
            store.insert(connection("conn", "2024-01-02")),
            Err(StoreError::ConnectionAlreadyExists { connection_id }) if connection_id == "conn"
        ));
        assert!(matches!(
            store.update(connection("missing", "2024-01-02")),
            Err(StoreError::ConnectionNotFound { connection_id }) if connection_id == "missing"
        ));

        let mut replacement = connection("conn", "2024-01-03");
        replacement.name = "renamed".to_owned();
        store.upsert(replacement)?;
        assert!(matches!(
            store.get("conn")?,
            Some(record) if record.name == "renamed"
        ));
        Ok(())
    }

    #[test]
    fn gateway_store_duplicate_missing_and_sorting() -> Result<(), Box<dyn Error + Send + Sync>> {
        let store = InMemoryGatewayStore::new();
        store.insert(gateway("later", "2024-01-02"))?;
        store.insert(gateway("earlier", "2024-01-01"))?;

        let ids = store
            .list()?
            .into_iter()
            .map(|record| record.gateway_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["earlier", "later"]);
        assert!(matches!(
            store.insert(gateway("earlier", "2024-01-03")),
            Err(StoreError::GatewayAlreadyExists { gateway_id }) if gateway_id == "earlier"
        ));
        assert!(matches!(
            store.update(gateway("missing", "2024-01-03")),
            Err(StoreError::GatewayNotFound { gateway_id }) if gateway_id == "missing"
        ));
        Ok(())
    }

    #[test]
    fn kernel_config_store_upserts_and_sorts_by_harness() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let store = InMemoryKernelConfigStore::new();
        store.upsert(HarnessName::Echo, "E=1")?;
        store.upsert(HarnessName::Acp, "A=1")?;
        store.upsert(HarnessName::Echo, "E=2")?;

        let records = store.list()?;
        let harnesses = records
            .iter()
            .map(|record| record.harness.as_str())
            .collect::<Vec<_>>();
        assert_eq!(harnesses, vec!["acp", "echo"]);
        assert!(matches!(
            store.get(HarnessName::Echo)?,
            Some(record) if record.env_vars == "E=2"
        ));
        assert!(store.delete(HarnessName::Acp)?);
        Ok(())
    }

    #[test]
    fn session_store_persists_updates_and_clears_messages()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let store = InMemorySessionStore::new();
        store.insert(session("later", "2024-01-02"))?;
        store.insert(session("session", "2024-01-01"))?;

        let ids = store
            .list()?
            .into_iter()
            .map(|record| record.session_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["session", "later"]);

        let message = MessageRecord::new("msg", "session", MessageRole::User, "hello");
        store.append_message(message)?;
        assert!(matches!(
            store.get("session")?,
            Some(record) if record.messages.len() == 1 && record.messages[0].content == "hello"
        ));

        let replacement = MessageRecord::new("msg", "session", MessageRole::Assistant, "updated");
        store.update_message(replacement)?;
        assert!(matches!(
            store.get("session")?,
            Some(record) if record.messages.len() == 1 && record.messages[0].content == "updated"
        ));

        let appended = MessageRecord::new("new", "session", MessageRole::Assistant, "new");
        store.update_message(appended)?;
        assert!(matches!(
            store.get("session")?,
            Some(record) if record.messages.len() == 2
        ));

        store.clear_messages("session")?;
        assert!(matches!(
            store.get("session")?,
            Some(record) if record.messages.is_empty()
        ));
        assert!(matches!(
            store.append_message(MessageRecord::new("missing", "missing", MessageRole::User, "nope")),
            Err(StoreError::SessionNotFound { session_id }) if session_id == "missing"
        ));
        Ok(())
    }

    #[test]
    fn session_store_reports_duplicate_and_missing_updates()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let store = InMemorySessionStore::new();
        store.insert(session("session", "2024-01-01"))?;
        assert!(matches!(
            store.insert(session("session", "2024-01-02")),
            Err(StoreError::SessionAlreadyExists { session_id }) if session_id == "session"
        ));
        assert!(matches!(
            store.update(session("missing", "2024-01-02")),
            Err(StoreError::SessionNotFound { session_id }) if session_id == "missing"
        ));
        assert!(store.delete("session")?);
        assert!(!store.delete("session")?);
        Ok(())
    }

    #[test]
    fn sqlite_stores_persist_records_across_reopen() -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = sqlite_test_path()?;
        {
            let stores = StoreSet::sqlite(&path)?;

            let mut connection = connection("conn", "2024-01-01");
            connection.api_flavor = ConnectionApiFlavor::Responses;
            connection.api_key = "secret".to_owned();
            stores.connections.insert(connection)?;

            let mut agent = agent("agent", "2024-01-02");
            agent.skills = vec!["skill-a".to_owned(), "skill-b".to_owned()];
            agent.env_vars = "A=B".to_owned();
            agent.connection_id = Some("conn".to_owned());
            stores.agents.insert(agent)?;

            stores.kernel_configs.upsert(HarnessName::Acp, "K=V")?;

            let mut gateway = gateway("gateway", "2024-01-03");
            gateway.enabled = true;
            gateway.env_vars = "G=V".to_owned();
            gateway
                .secrets
                .insert("TOKEN".to_owned(), "value".to_owned());
            gateway.status = "running".to_owned();
            gateway.container_name = Some("container".to_owned());
            stores.gateways.insert(gateway)?;

            let mut session = session("session", "2024-01-04");
            session.channel_name = Some("cli".to_owned());
            session.client_type = Some(ClientType::Cli);
            stores.sessions.insert(session)?;

            let mut message =
                MessageRecord::new("message", "session", MessageRole::Assistant, "hello");
            message.created_at = "2024-01-05".to_owned();
            message.reasoning = "because".to_owned();
            let mut tool_call = ToolCallRecord::new("shell");
            tool_call.tool_call_id = Some("call-1".to_owned());
            tool_call.status = Some("done".to_owned());
            tool_call.kind = Some("command".to_owned());
            tool_call.input = Some("echo hi".to_owned());
            tool_call.output = Some("hi".to_owned());
            tool_call.content_offset = Some(3);
            message.tool_calls.push(tool_call);
            stores.sessions.append_message(message)?;
        }

        {
            let stores = StoreSet::sqlite(&path)?;

            let connection =
                stores
                    .connections
                    .get("conn")?
                    .ok_or_else(|| StoreError::ConnectionNotFound {
                        connection_id: "conn".to_owned(),
                    })?;
            assert_eq!(connection.api_flavor, ConnectionApiFlavor::Responses);
            assert_eq!(connection.api_key, "secret");

            let agent = stores
                .agents
                .get("agent")?
                .ok_or_else(|| StoreError::AgentNotFound {
                    agent_id: "agent".to_owned(),
                })?;
            assert_eq!(
                agent.skills,
                vec!["skill-a".to_owned(), "skill-b".to_owned()]
            );
            assert_eq!(agent.connection_id.as_deref(), Some("conn"));

            let config = stores
                .kernel_configs
                .get(HarnessName::Acp)?
                .ok_or_else(|| StoreError::Persistence {
                    store: "kernel_configs",
                    detail: "missing acp config".to_owned(),
                })?;
            assert_eq!(config.env_vars, "K=V");

            let gateway =
                stores
                    .gateways
                    .get("gateway")?
                    .ok_or_else(|| StoreError::GatewayNotFound {
                        gateway_id: "gateway".to_owned(),
                    })?;
            assert!(gateway.enabled);
            assert_eq!(
                gateway.secrets.get("TOKEN").map(String::as_str),
                Some("value")
            );
            assert_eq!(gateway.container_name.as_deref(), Some("container"));

            let session =
                stores
                    .sessions
                    .get("session")?
                    .ok_or_else(|| StoreError::SessionNotFound {
                        session_id: "session".to_owned(),
                    })?;
            assert_eq!(session.client_type, Some(ClientType::Cli));
            assert_eq!(session.messages.len(), 1);
            let message = &session.messages[0];
            assert_eq!(message.reasoning, "because");
            assert_eq!(message.tool_calls.len(), 1);
            assert_eq!(message.tool_calls[0].tool, "shell");
            assert_eq!(message.tool_calls[0].content_offset, Some(3));
        }

        cleanup_sqlite_path(&path);
        Ok(())
    }
}
