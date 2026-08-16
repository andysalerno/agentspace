use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

mod sqlite;

use crate::{
    config::{adapter, state::ConfigState},
    errors::StoreError,
    models::{
        AgentRecord, ConnectionRecord, GatewayRecord, HarnessName, KernelConfigRecord,
        MessageRecord, SessionRecord, WorkspaceRecord,
    },
};

pub use sqlite::SqliteDatabase;

#[derive(Clone, Debug, Default)]
pub struct InMemoryWorkspaceStore {
    workspaces: Arc<RwLock<BTreeMap<String, WorkspaceRecord>>>,
}

impl InMemoryWorkspaceStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn list(&self) -> Result<Vec<WorkspaceRecord>, StoreError> {
        let mut records = with_read(&self.workspaces, "workspaces", |workspaces| {
            workspaces.values().cloned().collect::<Vec<_>>()
        })?;
        records.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.workspace_id.cmp(&right.workspace_id))
        });
        Ok(records)
    }

    pub fn get(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>, StoreError> {
        with_read(&self.workspaces, "workspaces", |workspaces| {
            workspaces.get(workspace_id).cloned()
        })
    }

    pub fn insert(&self, workspace: WorkspaceRecord) -> Result<(), StoreError> {
        with_write(&self.workspaces, "workspaces", |workspaces| {
            if workspaces.contains_key(&workspace.workspace_id) {
                return Err(StoreError::WorkspaceAlreadyExists {
                    workspace_id: workspace.workspace_id,
                });
            }
            workspaces.insert(workspace.workspace_id.clone(), workspace);
            Ok(())
        })?
    }

    pub fn update(&self, workspace: WorkspaceRecord) -> Result<(), StoreError> {
        with_write(&self.workspaces, "workspaces", |workspaces| {
            if !workspaces.contains_key(&workspace.workspace_id) {
                return Err(StoreError::WorkspaceNotFound {
                    workspace_id: workspace.workspace_id,
                });
            }
            workspaces.insert(workspace.workspace_id.clone(), workspace);
            Ok(())
        })?
    }

    pub fn delete(&self, workspace_id: &str) -> Result<bool, StoreError> {
        with_write(&self.workspaces, "workspaces", |workspaces| {
            workspaces.remove(workspace_id).is_some()
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

    pub fn append_message(&self, message: &MessageRecord) -> Result<(), StoreError> {
        with_write(&self.sessions, "sessions", |sessions| {
            let session = sessions.get_mut(&message.session_id).ok_or_else(|| {
                StoreError::SessionNotFound {
                    session_id: message.session_id.clone(),
                }
            })?;
            session.messages.push(message.clone());
            Ok(())
        })?
    }

    pub fn update_message(&self, message: &MessageRecord) -> Result<(), StoreError> {
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
                *existing = message.clone();
            } else {
                session.messages.push(message.clone());
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
    pub(crate) config: ConfigState,
    pub(crate) agents: AgentStore,
    pub(crate) kernel_configs: KernelConfigStore,
    pub(crate) connections: ConnectionStore,
    pub(crate) gateways: GatewayStore,
    pub(crate) workspaces: WorkspaceStore,
    pub(crate) sessions: SessionStore,
}

impl StoreSet {
    /// Build an in-memory store set backed by a single shared [`ConfigState`].
    ///
    /// # Errors
    /// Returns [`StoreError`] if the configuration state cannot be initialized.
    pub fn in_memory() -> Result<Self, StoreError> {
        let config = ConfigState::in_memory()?;
        Ok(Self {
            agents: AgentStore::new(config.clone()),
            kernel_configs: KernelConfigStore::new(config.clone()),
            connections: ConnectionStore::new(config.clone()),
            gateways: GatewayStore::new(config.clone()),
            workspaces: WorkspaceStore::in_memory(),
            sessions: SessionStore::in_memory(),
            config,
        })
    }

    /// Build a SQLite-backed store set. Configuration is persisted only through
    /// the opaque snapshot envelope and write-only secret store; runtime-only
    /// workspaces and sessions keep their dedicated tables.
    ///
    /// # Errors
    /// Returns [`StoreError`] on persistence or configuration failure.
    pub fn sqlite(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        let in_memory = path == Path::new(":memory:");
        tracing::info!(in_memory, "creating sqlite-backed store set");
        let stores = sqlite::SqliteStoreSet::open(path)?;
        let config = if in_memory {
            ConfigState::in_memory()?
        } else {
            let env: BTreeMap<String, String> = std::env::vars().collect();
            let path_str = path.to_str().ok_or_else(|| StoreError::Persistence {
                store: "config",
                detail: "database path is not valid UTF-8".to_owned(),
            })?;
            ConfigState::open(Some(path_str), &env)?
        };
        tracing::info!(in_memory, "created sqlite-backed store set");
        Ok(Self {
            agents: AgentStore::new(config.clone()),
            kernel_configs: KernelConfigStore::new(config.clone()),
            connections: ConnectionStore::new(config.clone()),
            gateways: GatewayStore::new(config.clone()),
            workspaces: WorkspaceStore::Sqlite(stores.workspaces),
            sessions: SessionStore::Sqlite(stores.sessions),
            config,
        })
    }
}

/// Agent store backed by the authoritative [`ConfigDocument`].
#[derive(Clone, Debug)]
pub struct AgentStore {
    config: ConfigState,
}

impl AgentStore {
    #[must_use]
    pub const fn new(config: ConfigState) -> Self {
        Self { config }
    }

    pub fn list(&self) -> Result<Vec<AgentRecord>, StoreError> {
        adapter::list_agents(&self.config)
    }

    pub fn get(&self, agent_id: &str) -> Result<Option<AgentRecord>, StoreError> {
        adapter::get_agent(&self.config, agent_id)
    }

    pub fn insert(&self, agent: &AgentRecord) -> Result<(), StoreError> {
        adapter::insert_agent(&self.config, agent)
    }

    pub fn update(&self, agent: &AgentRecord) -> Result<(), StoreError> {
        adapter::update_agent(&self.config, agent)
    }

    pub fn add_skill(&self, agent_id: &str, skill_id: &str) -> Result<bool, StoreError> {
        adapter::add_agent_skill(&self.config, agent_id, skill_id)
    }

    pub fn upsert(&self, agent: &AgentRecord) -> Result<(), StoreError> {
        adapter::upsert_agent(&self.config, agent)
    }

    pub fn delete(&self, agent_id: &str) -> Result<bool, StoreError> {
        adapter::delete_agent(&self.config, agent_id)
    }
}

/// Kernel configuration store backed by the authoritative document.
#[derive(Clone, Debug)]
pub struct KernelConfigStore {
    config: ConfigState,
}

impl KernelConfigStore {
    #[must_use]
    pub const fn new(config: ConfigState) -> Self {
        Self { config }
    }

    pub fn list(&self) -> Result<Vec<KernelConfigRecord>, StoreError> {
        adapter::list_kernel_configs(&self.config)
    }

    pub fn get(&self, harness: HarnessName) -> Result<Option<KernelConfigRecord>, StoreError> {
        adapter::get_kernel_config(&self.config, harness)
    }

    pub fn upsert(
        &self,
        harness: HarnessName,
        env_vars: impl Into<String>,
    ) -> Result<KernelConfigRecord, StoreError> {
        adapter::upsert_kernel_config(&self.config, harness, env_vars.into())
    }

    pub fn delete(&self, harness: HarnessName) -> Result<bool, StoreError> {
        adapter::delete_kernel_config(&self.config, harness)
    }
}

/// Connection store backed by the authoritative document.
#[derive(Clone, Debug)]
pub struct ConnectionStore {
    config: ConfigState,
}

impl ConnectionStore {
    #[must_use]
    pub const fn new(config: ConfigState) -> Self {
        Self { config }
    }

    pub fn list(&self) -> Result<Vec<ConnectionRecord>, StoreError> {
        adapter::list_connections(&self.config)
    }

    pub fn get(&self, connection_id: &str) -> Result<Option<ConnectionRecord>, StoreError> {
        adapter::get_connection(&self.config, connection_id)
    }

    pub fn insert(&self, connection: &ConnectionRecord) -> Result<(), StoreError> {
        adapter::insert_connection(&self.config, connection)
    }

    pub fn update(&self, connection: &ConnectionRecord) -> Result<(), StoreError> {
        adapter::update_connection(&self.config, connection)
    }

    pub fn upsert(&self, connection: &ConnectionRecord) -> Result<(), StoreError> {
        adapter::upsert_connection(&self.config, connection)
    }

    pub fn delete(&self, connection_id: &str) -> Result<bool, StoreError> {
        adapter::delete_connection(&self.config, connection_id)
    }
}

/// Gateway store backed by the authoritative document.
#[derive(Clone, Debug)]
pub struct GatewayStore {
    config: ConfigState,
}

impl GatewayStore {
    #[must_use]
    pub const fn new(config: ConfigState) -> Self {
        Self { config }
    }

    pub fn list(&self) -> Result<Vec<GatewayRecord>, StoreError> {
        adapter::list_gateways(&self.config)
    }

    pub fn get(&self, gateway_id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        adapter::get_gateway(&self.config, gateway_id)
    }

    pub fn insert(&self, gateway: &GatewayRecord) -> Result<(), StoreError> {
        adapter::insert_gateway(&self.config, gateway)
    }

    pub fn update(&self, gateway: &GatewayRecord) -> Result<(), StoreError> {
        adapter::update_gateway(&self.config, gateway)
    }

    /// Update only the observed runtime status (never the desired config).
    pub fn set_runtime_status(
        &self,
        gateway_id: &str,
        status: &str,
        last_error: Option<String>,
        container_name: Option<String>,
    ) {
        adapter::set_gateway_runtime_status(
            &self.config,
            gateway_id,
            crate::config::state::GatewayRuntime {
                status: status.to_owned(),
                last_error,
                container_name,
            },
        );
    }

    pub fn upsert(&self, gateway: &GatewayRecord) -> Result<(), StoreError> {
        adapter::upsert_gateway(&self.config, gateway)
    }

    pub fn delete(&self, gateway_id: &str) -> Result<bool, StoreError> {
        adapter::delete_gateway(&self.config, gateway_id)
    }
}

#[derive(Clone, Debug)]
pub enum WorkspaceStore {
    InMemory(InMemoryWorkspaceStore),
    Sqlite(sqlite::SqliteWorkspaceStore),
}

impl WorkspaceStore {
    #[must_use]
    pub fn in_memory() -> Self {
        Self::InMemory(InMemoryWorkspaceStore::new())
    }

    pub fn list(&self) -> Result<Vec<WorkspaceRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.list(),
            Self::Sqlite(store) => store.list(),
        }
    }

    pub fn get(&self, workspace_id: &str) -> Result<Option<WorkspaceRecord>, StoreError> {
        match self {
            Self::InMemory(store) => store.get(workspace_id),
            Self::Sqlite(store) => store.get(workspace_id),
        }
    }

    pub fn insert(&self, workspace: WorkspaceRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.insert(workspace),
            Self::Sqlite(store) => store.insert(workspace),
        }
    }

    pub fn update(&self, workspace: WorkspaceRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.update(workspace),
            Self::Sqlite(store) => store.update(workspace),
        }
    }

    pub fn delete(&self, workspace_id: &str) -> Result<bool, StoreError> {
        match self {
            Self::InMemory(store) => store.delete(workspace_id),
            Self::Sqlite(store) => store.delete(workspace_id),
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

    pub fn append_message(&self, message: &MessageRecord) -> Result<(), StoreError> {
        match self {
            Self::InMemory(store) => store.append_message(message),
            Self::Sqlite(store) => store.append_message(message),
        }
    }

    pub fn update_message(&self, message: &MessageRecord) -> Result<(), StoreError> {
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
    lock.read().map_err(|_error| {
        tracing::warn!(store, lock = "read", "store lock poisoned");
        StoreError::LockPoisoned { store }
    })
}

fn write_lock<'a, T>(
    lock: &'a RwLock<T>,
    store: &'static str,
) -> Result<RwLockWriteGuard<'a, T>, StoreError> {
    lock.write().map_err(|_error| {
        tracing::warn!(store, lock = "write", "store lock poisoned");
        StoreError::LockPoisoned { store }
    })
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
            AdditionalPathIdentity, AgentCliRecord, AgentRecord, CliHarnessName,
            CliLaunchOptionsSnapshot, CliLaunchSnapshot, ClientType, ConnectionApiFlavor,
            ConnectionRecord, GatewayRecord, GatewayType, HarnessName, InteractionMode,
            MessageRecord, MessageRole, RuntimeStatus, SessionRecord, ToolCallRecord,
        },
    };

    use super::{
        AgentStore, ConnectionStore, GatewayStore, InMemorySessionStore, KernelConfigStore,
        StoreSet,
    };
    use crate::config::state::ConfigState;

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
        let store = AgentStore::new(ConfigState::in_memory()?);
        store.insert(&agent("second", "2024-01-02"))?;
        store.insert(&agent("first", "2024-01-01"))?;

        let ids = store
            .list()?
            .into_iter()
            .map(|record| record.agent_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["first", "second"]);

        let duplicate = store.insert(&agent("first", "2024-01-03"));
        assert!(matches!(
            duplicate,
            Err(StoreError::AgentAlreadyExists { agent_id }) if agent_id == "first"
        ));

        let missing = store.update(&agent("missing", "2024-01-03"));
        assert!(matches!(
            missing,
            Err(StoreError::AgentNotFound { agent_id }) if agent_id == "missing"
        ));
        assert!(store.delete("first")?);
        assert!(!store.delete("first")?);
        Ok(())
    }

    #[test]
    fn agent_store_add_skill_preserves_other_fields_and_deduplicates()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let store = AgentStore::new(ConfigState::in_memory()?);
        let mut record = agent("agent", "2024-01-01");
        record.name = "Original Name".to_owned();
        record.env_vars = "A=B".to_owned();
        store.insert(&record)?;

        assert!(store.add_skill("agent", "new-skill")?);
        assert!(!store.add_skill("agent", "new-skill")?);
        let updated = store
            .get("agent")?
            .ok_or_else(|| StoreError::AgentNotFound {
                agent_id: "agent".to_owned(),
            })?;

        assert_eq!(updated.name, "Original Name");
        assert_eq!(updated.env_vars, "A=B");
        assert_eq!(updated.skills, vec!["new-skill".to_owned()]);
        assert!(matches!(
            store.add_skill("missing", "new-skill"),
            Err(StoreError::AgentNotFound { agent_id }) if agent_id == "missing"
        ));
        Ok(())
    }

    #[test]
    fn connection_store_duplicate_missing_and_upsert() -> Result<(), Box<dyn Error + Send + Sync>> {
        let store = ConnectionStore::new(ConfigState::in_memory()?);
        store.insert(&connection("conn", "2024-01-01"))?;
        assert!(matches!(
            store.insert(&connection("conn", "2024-01-02")),
            Err(StoreError::ConnectionAlreadyExists { connection_id }) if connection_id == "conn"
        ));
        assert!(matches!(
            store.update(&connection("missing", "2024-01-02")),
            Err(StoreError::ConnectionNotFound { connection_id }) if connection_id == "missing"
        ));

        let mut replacement = connection("conn", "2024-01-03");
        replacement.name = "renamed".to_owned();
        store.upsert(&replacement)?;
        assert!(matches!(
            store.get("conn")?,
            Some(record) if record.name == "renamed"
        ));
        Ok(())
    }

    #[test]
    fn gateway_store_duplicate_missing_and_sorting() -> Result<(), Box<dyn Error + Send + Sync>> {
        let config = ConfigState::in_memory()?;
        AgentStore::new(config.clone()).insert(&agent("agent", "2024-01-01"))?;
        let store = GatewayStore::new(config);
        store.insert(&gateway("later", "2024-01-02"))?;
        store.insert(&gateway("earlier", "2024-01-01"))?;

        let ids = store
            .list()?
            .into_iter()
            .map(|record| record.gateway_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["earlier", "later"]);
        assert!(matches!(
            store.insert(&gateway("earlier", "2024-01-03")),
            Err(StoreError::GatewayAlreadyExists { gateway_id }) if gateway_id == "earlier"
        ));
        assert!(matches!(
            store.update(&gateway("missing", "2024-01-03")),
            Err(StoreError::GatewayNotFound { gateway_id }) if gateway_id == "missing"
        ));
        Ok(())
    }

    #[test]
    fn kernel_config_store_upserts_and_sorts_by_harness() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let store = KernelConfigStore::new(ConfigState::in_memory()?);
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
        store.append_message(&message)?;
        assert!(matches!(
            store.get("session")?,
            Some(record) if record.messages.len() == 1 && record.messages[0].content == "hello"
        ));

        let replacement = MessageRecord::new("msg", "session", MessageRole::Assistant, "updated");
        store.update_message(&replacement)?;
        assert!(matches!(
            store.get("session")?,
            Some(record) if record.messages.len() == 1 && record.messages[0].content == "updated"
        ));

        let appended = MessageRecord::new("new", "session", MessageRole::Assistant, "new");
        store.update_message(&appended)?;
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
            store.append_message(&MessageRecord::new("missing", "missing", MessageRole::User, "nope")),
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
    #[allow(clippy::too_many_lines)]
    fn sqlite_stores_persist_records_across_reopen() -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = sqlite_test_path()?;
        {
            let stores = StoreSet::sqlite(&path)?;

            let mut connection = connection("conn", "2024-01-01");
            connection.api_flavor = ConnectionApiFlavor::Responses;
            connection.api_key = "secret".to_owned();
            stores.connections.insert(&connection)?;

            let mut agent = agent("agent", "2024-01-02");
            agent.skills = vec!["skill-a".to_owned(), "skill-b".to_owned()];
            agent.env_vars = "A=B".to_owned();
            agent.connection_id = Some("conn".to_owned());
            agent.cli = Some(AgentCliRecord {
                harness: CliHarnessName::CopilotCli,
                connection_id: Some("conn".to_owned()),
            });
            stores.agents.insert(&agent)?;

            stores.kernel_configs.upsert(HarnessName::Acp, "K=V")?;

            let mut gateway = gateway("gateway", "2024-01-03");
            gateway.enabled = true;
            gateway.env_vars = "G=V".to_owned();
            gateway
                .secrets
                .insert("TOKEN".to_owned(), "value".to_owned());
            gateway.status = "running".to_owned();
            gateway.container_name = Some("container".to_owned());
            stores.gateways.insert(&gateway)?;

            let mut session = session("session", "2024-01-04");
            session.channel_name = Some("cli".to_owned());
            session.client_type = Some(ClientType::Cli);
            session.interaction_mode = InteractionMode::Cli;
            session.cli_harness = Some(CliHarnessName::CopilotCli);
            session.cli_connection_id = Some("conn".to_owned());
            session.harness_session_id = Some("73d5eb10-cac7-44ca-8aa8-d41da5a24f13".to_owned());
            session.runtime_generation = Some(2);
            session.runtime_status = Some(RuntimeStatus::Starting);
            session.workspace_volume_identity = Some("workspace-identity".to_owned());
            session.launch_snapshot = Some(CliLaunchSnapshot {
                schema_version: 1,
                provider: None,
                model: None,
                reasoning_effort: None,
                options: CliLaunchOptionsSnapshot {
                    no_auto_update: true,
                    mouse: true,
                    config_dir: None,
                    extra_args: None,
                },
                additional_paths: vec![AdditionalPathIdentity::SessionWorkspace {
                    path: "/workspace".to_owned(),
                }],
                agent_profile: None,
            });
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
            stores.sessions.append_message(&message)?;
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
            assert_eq!(
                agent.cli,
                Some(AgentCliRecord {
                    harness: CliHarnessName::CopilotCli,
                    connection_id: Some("conn".to_owned()),
                })
            );

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
            assert_eq!(gateway.container_name, None);

            let session =
                stores
                    .sessions
                    .get("session")?
                    .ok_or_else(|| StoreError::SessionNotFound {
                        session_id: "session".to_owned(),
                    })?;
            assert_eq!(session.client_type, Some(ClientType::Cli));
            assert_eq!(session.interaction_mode, InteractionMode::Cli);
            assert_eq!(session.cli_harness, Some(CliHarnessName::CopilotCli));
            assert_eq!(session.cli_connection_id.as_deref(), Some("conn"));
            assert_eq!(session.runtime_generation, Some(2));
            assert_eq!(session.runtime_status, Some(RuntimeStatus::Starting));
            assert_eq!(
                session.workspace_volume_identity.as_deref(),
                Some("workspace-identity")
            );
            assert_eq!(
                session
                    .launch_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.schema_version),
                Some(1)
            );
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

    #[test]
    fn sqlite_migrates_legacy_sessions_to_chat() -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = sqlite_test_path()?;
        {
            let connection = rusqlite::Connection::open(&path)?;
            connection.execute_batch(
                "
                CREATE TABLE client_sessions (
                    session_id TEXT PRIMARY KEY,
                    agent_id TEXT NOT NULL,
                    agent_host_session_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    channel_name TEXT,
                    client_type TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                INSERT INTO client_sessions (
                    session_id, agent_id, agent_host_session_id, status,
                    channel_name, client_type, created_at, updated_at
                ) VALUES (
                    'legacy', 'agent', 'host-session', 'idle',
                    NULL, 'webui', '2024-01-01', '2024-01-01'
                );
                ",
            )?;
        }

        let stores = StoreSet::sqlite(&path)?;
        let session =
            stores
                .sessions
                .get("legacy")?
                .ok_or_else(|| StoreError::SessionNotFound {
                    session_id: "legacy".to_owned(),
                })?;
        assert_eq!(session.interaction_mode, InteractionMode::Chat);
        assert_eq!(session.cli_harness, None);
        assert_eq!(session.launch_snapshot, None);

        cleanup_sqlite_path(&path);
        Ok(())
    }

    #[test]
    fn sqlite_agent_store_add_skill_preserves_current_fields()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = sqlite_test_path()?;
        {
            let stores = StoreSet::sqlite(&path)?;
            stores.agents.insert(&agent("agent", "2024-01-01"))?;

            let mut changed =
                stores
                    .agents
                    .get("agent")?
                    .ok_or_else(|| StoreError::AgentNotFound {
                        agent_id: "agent".to_owned(),
                    })?;
            changed.name = "Renamed Agent".to_owned();
            changed.env_vars = "SHARED=updated".to_owned();
            stores.agents.update(&changed)?;

            assert!(stores.agents.add_skill("agent", "new-skill")?);
            assert!(!stores.agents.add_skill("agent", "new-skill")?);
            let updated = stores
                .agents
                .get("agent")?
                .ok_or_else(|| StoreError::AgentNotFound {
                    agent_id: "agent".to_owned(),
                })?;
            assert_eq!(updated.name, "Renamed Agent");
            assert_eq!(updated.env_vars, "SHARED=updated");
            assert_eq!(updated.skills, vec!["new-skill".to_owned()]);
        }

        cleanup_sqlite_path(&path);
        Ok(())
    }
}
