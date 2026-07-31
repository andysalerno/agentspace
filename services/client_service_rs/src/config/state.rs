use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, RwLock},
};

use crate::{
    config::{
        bundle::load_bundle,
        canonical::{sha256_hex, to_canonical_yaml, typed_equal},
        document::ConfigDocument,
        error::ConfigError,
        loader::load_yaml,
        plan::{Plan, plan},
        secrets::SecretStore,
        snapshot::{Snapshot, SnapshotStore, SourceKind},
        validate::{secret_references, validate, validate_mutation},
    },
    errors::StoreError,
    models::{WorkspaceMountRecord, utc_now},
};

/// Parse complete-replacement source bytes into a document, according to the
/// declared source kind (plain YAML or a config-set zip bundle).
fn parse_source(source: &[u8], kind: SourceKind) -> Result<ConfigDocument, ConfigError> {
    match kind {
        SourceKind::Yaml => {
            let text = std::str::from_utf8(source).map_err(|error| ConfigError::Parse {
                detail: error.to_string(),
            })?;
            load_yaml(text)
        }
        SourceKind::Bundle => load_bundle(source),
    }
}

/// Runtime, disposable metadata that is not part of the desired-state document:
/// synthesized timestamps, observed gateway status, and agent workspace mounts.
#[derive(Default)]
struct RuntimeMeta {
    timestamps: BTreeMap<String, (String, String)>,
    gateways: BTreeMap<String, GatewayRuntime>,
    agent_mounts: BTreeMap<String, Vec<WorkspaceMountRecord>>,
}

/// Observed runtime state for a gateway container.
#[derive(Clone, Debug)]
pub struct GatewayRuntime {
    pub status: String,
    pub last_error: Option<String>,
    pub container_name: Option<String>,
}

impl Default for GatewayRuntime {
    fn default() -> Self {
        Self {
            status: "stopped".to_owned(),
            last_error: None,
            container_name: None,
        }
    }
}

/// The result of a complete-replacement apply.
#[derive(Clone, Debug)]
pub struct ApplyOutcome {
    pub snapshot: Snapshot,
    pub plan: Plan,
    pub document: Arc<ConfigDocument>,
    pub unset_secrets: Vec<String>,
}

/// A validated, not-yet-committed complete-replacement apply.
///
/// Preparing an apply validates the graph and computes the plan without
/// mutating state, so the caller can stage external reconciliation (skill
/// materialization) before the snapshot is atomically activated by
/// [`ConfigState::commit`].
#[derive(Clone, Debug)]
pub struct PreparedApply {
    pub document: Arc<ConfigDocument>,
    pub plan: Plan,
    pub unset_secrets: Vec<String>,
    /// User skills declared by the previously active document, retained so the
    /// caller can compute the skill reconciliation diff and compensate.
    pub previous_skills: Vec<crate::config::document::Skill>,
    source_bytes: Vec<u8>,
    kind: SourceKind,
    semantic: String,
    expected_generation: Option<i64>,
}

struct Inner {
    active: RwLock<Arc<ConfigDocument>>,
    snapshots: SnapshotStore,
    secrets: Arc<SecretStore>,
    runtime: RwLock<RuntimeMeta>,
    /// Serializes secret declaration changes and value set/clear operations so
    /// applies and value writes cannot race and orphan a value.
    secret_ops: Mutex<()>,
}

/// The authoritative, shared configuration state. Every config store adapter,
/// route, runtime consumer, graph validator, and UI mutation reads and mutates
/// this single document snapshot.
#[derive(Clone)]
pub struct ConfigState {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ConfigState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigState")
            .finish_non_exhaustive()
    }
}

impl ConfigState {
    /// Build an in-memory config state with an ephemeral secret key.
    ///
    /// # Errors
    /// Returns [`StoreError`] if the secret store cannot be initialized.
    pub fn in_memory() -> Result<Self, StoreError> {
        let secrets =
            SecretStore::open(None, &BTreeMap::new()).map_err(|error| secret_error(&error))?;
        Ok(Self {
            inner: Arc::new(Inner {
                active: RwLock::new(Arc::new(ConfigDocument::default())),
                snapshots: SnapshotStore::in_memory(),
                secrets,
                runtime: RwLock::new(RuntimeMeta::default()),
                secret_ops: Mutex::new(()),
            }),
        })
    }

    /// Open a config state, backing the snapshot envelope and secret store with
    /// `SQLite` when `db_path` is set. On startup the active snapshot source is
    /// loaded and published as the immutable in-memory document.
    ///
    /// # Errors
    /// Returns [`StoreError`] on persistence, legacy-schema, or master-key
    /// failures.
    pub fn open(db_path: Option<&str>, env: &BTreeMap<String, String>) -> Result<Self, StoreError> {
        let snapshots = match db_path {
            Some(path) => SnapshotStore::sqlite(path)?,
            None => SnapshotStore::in_memory(),
        };
        let active_snapshot = snapshots.active()?;
        if let Some(path) = db_path {
            guard_against_legacy_tables(path, active_snapshot.is_some())?;
        }
        let secrets = SecretStore::open(db_path, env).map_err(|error| secret_error(&error))?;
        let active = match active_snapshot {
            Some(snapshot) => {
                let kind = source_kind_from_str(&snapshot.source_kind);
                let document =
                    parse_source(&snapshot.source_bytes, kind).map_err(|e| config_to_store(&e))?;
                Arc::new(document)
            }
            None => Arc::new(ConfigDocument::default()),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                active: RwLock::new(active),
                snapshots,
                secrets,
                runtime: RwLock::new(RuntimeMeta::default()),
                secret_ops: Mutex::new(()),
            }),
        })
    }

    /// Return the active immutable document snapshot.
    #[must_use]
    pub fn active(&self) -> Arc<ConfigDocument> {
        self.inner.active.read().map_or_else(
            |poisoned| poisoned.into_inner().clone(),
            |guard| guard.clone(),
        )
    }

    /// Return the shared secret store.
    #[must_use]
    pub fn secrets(&self) -> Arc<SecretStore> {
        self.inner.secrets.clone()
    }

    /// Return the active snapshot envelope, if any generation has been applied.
    ///
    /// # Errors
    /// Returns [`StoreError`] on persistence failure.
    pub fn active_snapshot(&self) -> Result<Option<Snapshot>, StoreError> {
        self.inner.snapshots.active()
    }

    /// Clone the active document, apply a mutation, validate, canonically
    /// regenerate the source, and install it as the next snapshot. A mutation
    /// that does not change configuration content is a no-op (no new snapshot).
    ///
    /// # Errors
    /// Propagates the closure's [`StoreError`] or a validation/persistence
    /// failure.
    pub fn mutate<F>(&self, mutation: F) -> Result<(), StoreError>
    where
        F: FnOnce(&mut ConfigDocument) -> Result<(), StoreError>,
    {
        let mut guard = self
            .inner
            .active
            .write()
            .map_err(|_| StoreError::LockPoisoned { store: "config" })?;
        let current = guard.clone();
        let mut next = (*current).clone();
        mutation(&mut next)?;
        if typed_equal(&next, &current) {
            return Ok(());
        }
        validate_mutation(&next).map_err(|e| config_to_store(&e))?;
        let canonical = to_canonical_yaml(&next).map_err(|e| config_to_store(&e))?;
        let semantic = sha256_hex(canonical.as_bytes());
        self.inner.snapshots.insert_and_activate(
            SourceKind::Yaml,
            canonical.into_bytes(),
            semantic,
        )?;
        *guard = Arc::new(next);
        drop(guard);
        Ok(())
    }

    /// Apply a complete-replacement source document in a single call. Validates
    /// the graph, blocks removal of declarations with set values, asserts the
    /// canonical projection round-trips, persists the exact source bytes, and
    /// publishes the new immutable document.
    ///
    /// This is a convenience wrapper over [`ConfigState::prepare`] +
    /// [`ConfigState::commit`] with no optimistic-concurrency check and no
    /// external reconciliation. Callers that must reconcile `agent_host` should
    /// use `prepare`/`commit` so skills can be staged before activation.
    ///
    /// `builtin_skill_ids` are installation-owned skill IDs that agents may
    /// reference in addition to skills declared in the document.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] on parse, validation, secret-omission, or
    /// persistence failure.
    pub fn apply(
        &self,
        source: &[u8],
        kind: SourceKind,
        builtin_skill_ids: &BTreeSet<String>,
    ) -> Result<ApplyOutcome, ConfigError> {
        let prepared = self.prepare(source, kind, builtin_skill_ids, None)?;
        self.commit(prepared)
    }

    /// The active snapshot generation, if any generation has been applied.
    #[must_use]
    pub fn active_generation(&self) -> Option<i64> {
        self.active_snapshot().ok().flatten().map(|s| s.generation)
    }

    /// Validate and plan a complete-replacement source document without mutating
    /// state, returning a [`PreparedApply`] the caller commits after staging
    /// external reconciliation.
    ///
    /// When `expected_generation` is `Some`, the active generation is checked
    /// eagerly here (fail-fast) and again atomically at
    /// [`ConfigState::commit`].
    ///
    /// # Errors
    /// Returns a [`ConfigError`] on parse, validation, secret-omission,
    /// generation-conflict, or serialization failure.
    pub fn prepare(
        &self,
        source: &[u8],
        kind: SourceKind,
        builtin_skill_ids: &BTreeSet<String>,
        expected_generation: Option<i64>,
    ) -> Result<PreparedApply, ConfigError> {
        let next = parse_source(source, kind)?;
        validate(&next, builtin_skill_ids)?;

        let canonical = to_canonical_yaml(&next)?;
        let reparsed = load_yaml(&canonical)?;
        if !typed_equal(&reparsed, &next) {
            return Err(ConfigError::CanonicalDrift);
        }
        let semantic = sha256_hex(canonical.as_bytes());

        if let Some(expected) = expected_generation {
            let actual = self.active_generation();
            if actual != Some(expected) {
                return Err(ConfigError::GenerationConflict { expected, actual });
            }
        }
        self.check_secret_removal(&next)?;

        let current = self.active();
        let plan = plan(&current, &next);
        let unset_secrets = self.unset_referenced_secrets(&next);
        Ok(PreparedApply {
            document: Arc::new(next),
            plan,
            unset_secrets,
            previous_skills: current.spec.skills.clone(),
            source_bytes: source.to_vec(),
            kind,
            semantic,
            expected_generation,
        })
    }

    /// Atomically activate a [`PreparedApply`]: re-check the secret-removal and
    /// optimistic-concurrency invariants under the config lock, persist the
    /// exact source bytes, and publish the new immutable document.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] on secret-omission, generation-conflict, or
    /// persistence failure.
    pub fn commit(&self, prepared: PreparedApply) -> Result<ApplyOutcome, ConfigError> {
        // Serialize the whole commit against interleaved secret set/clear/apply
        // operations so a declaration removal cannot race an orphaning value
        // write.
        let _secret_guard = self
            .inner
            .secret_ops
            .lock()
            .map_err(|_| ConfigError::Serialize {
                detail: "secret operation lock poisoned".to_owned(),
            })?;
        self.check_secret_removal(&prepared.document)?;

        let mut guard = self
            .inner
            .active
            .write()
            .map_err(|_| ConfigError::Serialize {
                detail: "config lock poisoned".to_owned(),
            })?;
        // Optimistic concurrency: re-check the active generation atomically
        // under the config write lock so a stale apply cannot clobber an
        // intervening mutation.
        if let Some(expected) = prepared.expected_generation {
            let actual = self.active_generation();
            if actual != Some(expected) {
                return Err(ConfigError::GenerationConflict { expected, actual });
            }
        }
        let current = guard.clone();
        let plan = plan(&current, &prepared.document);
        let snapshot = self
            .inner
            .snapshots
            .insert_and_activate(prepared.kind, prepared.source_bytes, prepared.semantic)
            .map_err(|error| ConfigError::Serialize {
                detail: error.to_string(),
            })?;
        let document = prepared.document;
        *guard = document.clone();
        drop(guard);

        Ok(ApplyOutcome {
            snapshot,
            plan,
            document,
            unset_secrets: prepared.unset_secrets,
        })
    }

    /// Plan a source document against the active configuration without applying.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] on parse or validation failure.
    pub fn plan_source(
        &self,
        source: &[u8],
        kind: SourceKind,
        builtin_skill_ids: &BTreeSet<String>,
    ) -> Result<(Plan, Vec<String>), ConfigError> {
        let next = parse_source(source, kind)?;
        validate(&next, builtin_skill_ids)?;
        let current = self.active();
        let plan = plan(&current, &next);
        let unset = self.unset_referenced_secrets(&next);
        Ok((plan, unset))
    }

    /// Validate a source document without mutating any state.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] on parse or validation failure.
    pub fn validate_source(
        &self,
        source: &[u8],
        kind: SourceKind,
        builtin_skill_ids: &BTreeSet<String>,
    ) -> Result<Vec<String>, ConfigError> {
        let next = parse_source(source, kind)?;
        validate(&next, builtin_skill_ids)?;
        Ok(self.unset_referenced_secrets(&next))
    }

    /// The exact active source bytes, or the canonical projection of the
    /// current document if no snapshot has been persisted yet.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] if canonical serialization fails.
    pub fn source_bytes(&self) -> Result<Vec<u8>, ConfigError> {
        match self
            .active_snapshot()
            .map_err(|error| ConfigError::Serialize {
                detail: error.to_string(),
            })? {
            Some(snapshot) => Ok(snapshot.source_bytes),
            None => Ok(to_canonical_yaml(&self.active())?.into_bytes()),
        }
    }

    /// The exact active source bytes plus the source kind, so exports can serve
    /// the right content type. Falls back to canonical YAML when no snapshot
    /// exists yet.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] if canonical serialization fails.
    pub fn source_export(&self) -> Result<(Vec<u8>, SourceKind), ConfigError> {
        match self
            .active_snapshot()
            .map_err(|error| ConfigError::Serialize {
                detail: error.to_string(),
            })? {
            Some(snapshot) => {
                let kind = source_kind_from_str(&snapshot.source_kind);
                Ok((snapshot.source_bytes, kind))
            }
            None => Ok((
                to_canonical_yaml(&self.active())?.into_bytes(),
                SourceKind::Yaml,
            )),
        }
    }

    fn check_secret_removal(&self, next: &ConfigDocument) -> Result<(), ConfigError> {
        let current = self.active();
        let set_values =
            self.inner
                .secrets
                .set_names()
                .map_err(|error| ConfigError::Serialize {
                    detail: error.to_string(),
                })?;
        let mut blocked = Vec::new();
        for declaration in &current.spec.secrets {
            let name = declaration.name.as_str();
            if set_values.contains(name) && !next.secret_declared(name) {
                blocked.push(name.to_owned());
            }
        }
        if blocked.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::SecretDeclarationRemovalBlocked { names: blocked })
        }
    }

    fn unset_referenced_secrets(&self, document: &ConfigDocument) -> Vec<String> {
        let set_values = self.inner.secrets.set_names().unwrap_or_default();
        let mut unset: BTreeSet<String> = BTreeSet::new();
        for reference in secret_references(document) {
            if !set_values.contains(reference.name.as_str()) {
                unset.insert(reference.name.into_string());
            }
        }
        unset.into_iter().collect()
    }

    /// Count how many document fields reference each declared secret.
    #[must_use]
    pub fn secret_reference_fields(&self, name: &str) -> Vec<String> {
        secret_references(&self.active())
            .into_iter()
            .filter(|reference| reference.name.as_str() == name)
            .map(|reference| reference.field)
            .collect()
    }

    // ----- runtime metadata (disposable, not part of the document) -----

    /// Ensure timestamps exist for a runtime key and return `(created, updated)`.
    #[must_use]
    pub fn timestamps(&self, key: &str) -> (String, String) {
        if let Ok(mut runtime) = self.inner.runtime.write() {
            return runtime
                .timestamps
                .entry(key.to_owned())
                .or_insert_with(|| {
                    let now = utc_now();
                    (now.clone(), now)
                })
                .clone();
        }
        let now = utc_now();
        (now.clone(), now)
    }

    /// Record creation timestamps for a runtime key.
    pub fn mark_created(&self, key: &str) {
        if let Ok(mut runtime) = self.inner.runtime.write() {
            let now = utc_now();
            runtime
                .timestamps
                .insert(key.to_owned(), (now.clone(), now));
        }
    }

    /// Bump the updated timestamp for a runtime key, preserving creation time.
    pub fn mark_updated(&self, key: &str) {
        if let Ok(mut runtime) = self.inner.runtime.write() {
            let now = utc_now();
            runtime
                .timestamps
                .entry(key.to_owned())
                .and_modify(|entry| entry.1.clone_from(&now))
                .or_insert_with(|| (now.clone(), now));
        }
    }

    /// Remove all runtime metadata for a key.
    pub fn remove_meta(&self, key: &str) {
        if let Ok(mut runtime) = self.inner.runtime.write() {
            runtime.timestamps.remove(key);
        }
    }

    /// Observed runtime state for a gateway.
    #[must_use]
    pub fn gateway_runtime(&self, gateway_id: &str) -> GatewayRuntime {
        self.inner
            .runtime
            .read()
            .ok()
            .and_then(|runtime| runtime.gateways.get(gateway_id).cloned())
            .unwrap_or_default()
    }

    /// Replace observed runtime state for a gateway.
    pub fn set_gateway_runtime(&self, gateway_id: &str, runtime_state: GatewayRuntime) {
        if let Ok(mut runtime) = self.inner.runtime.write() {
            runtime
                .gateways
                .insert(gateway_id.to_owned(), runtime_state);
        }
    }

    /// Remove observed runtime state for a gateway.
    pub fn remove_gateway_runtime(&self, gateway_id: &str) {
        if let Ok(mut runtime) = self.inner.runtime.write() {
            runtime.gateways.remove(gateway_id);
        }
    }

    /// Agent workspace mounts (runtime binding, excluded from config).
    #[must_use]
    pub fn agent_mounts(&self, agent_id: &str) -> Vec<WorkspaceMountRecord> {
        self.inner
            .runtime
            .read()
            .ok()
            .and_then(|runtime| runtime.agent_mounts.get(agent_id).cloned())
            .unwrap_or_default()
    }

    /// Set agent workspace mounts.
    pub fn set_agent_mounts(&self, agent_id: &str, mounts: Vec<WorkspaceMountRecord>) {
        if let Ok(mut runtime) = self.inner.runtime.write() {
            if mounts.is_empty() {
                runtime.agent_mounts.remove(agent_id);
            } else {
                runtime.agent_mounts.insert(agent_id.to_owned(), mounts);
            }
        }
    }

    // ----- serialized secret operations -----

    fn lock_secret_ops(&self) -> Result<std::sync::MutexGuard<'_, ()>, StoreError> {
        self.inner
            .secret_ops
            .lock()
            .map_err(|_| StoreError::LockPoisoned {
                store: "secret_ops",
            })
    }

    /// Declare a new secret. Serialized against applies and value operations.
    /// Returns `false` when the declaration already exists.
    ///
    /// # Errors
    /// Returns [`StoreError`] on lock or persistence failure.
    pub fn declare_secret(
        &self,
        declaration: crate::config::document::SecretDeclaration,
    ) -> Result<bool, StoreError> {
        let _guard = self.lock_secret_ops()?;
        if self.active().secret_declared(declaration.name.as_str()) {
            return Ok(false);
        }
        self.mutate(move |document| {
            document.spec.secrets.push(declaration);
            Ok(())
        })?;
        Ok(true)
    }

    /// Attempt to remove a secret declaration. Serialized against applies and
    /// value operations; rechecks value/reference state immediately before the
    /// commit so a value cannot be orphaned.
    ///
    /// # Errors
    /// Returns [`StoreError`] on lock or persistence failure.
    pub fn undeclare_secret(&self, name: &str) -> Result<SecretRemoval, StoreError> {
        let _guard = self.lock_secret_ops()?;
        if !self.active().secret_declared(name) {
            return Ok(SecretRemoval::NotDeclared);
        }
        if self
            .inner
            .secrets
            .is_set(name)
            .map_err(|error| secret_error(&error))?
        {
            return Ok(SecretRemoval::ValueSet);
        }
        let references = self.secret_reference_fields(name);
        if !references.is_empty() {
            return Ok(SecretRemoval::Referenced(references));
        }
        let target = name.to_owned();
        self.mutate(move |document| {
            document
                .spec
                .secrets
                .retain(|item| item.name.as_str() != target);
            Ok(())
        })?;
        Ok(SecretRemoval::Removed)
    }

    /// Set or replace a secret value. Serialized against applies and removals;
    /// the declaration is rechecked under the lock so a value cannot be written
    /// for a declaration that a concurrent apply is removing.
    ///
    /// # Errors
    /// Returns [`StoreError`] on lock or persistence failure.
    pub fn set_secret_value(&self, name: &str, value: &str) -> Result<bool, StoreError> {
        let _guard = self.lock_secret_ops()?;
        if !self.active().secret_declared(name) {
            return Ok(false);
        }
        self.inner
            .secrets
            .set_value(name, value)
            .map_err(|error| secret_error(&error))?;
        Ok(true)
    }

    /// Clear a secret value. Serialized against applies and removals. Returns
    /// whether a value existed.
    ///
    /// # Errors
    /// Returns [`StoreError`] on lock or persistence failure.
    pub fn clear_secret_value(&self, name: &str) -> Result<bool, StoreError> {
        let _guard = self.lock_secret_ops()?;
        self.inner
            .secrets
            .clear_value(name)
            .map_err(|error| secret_error(&error))
    }
}

/// The outcome of attempting to remove a secret declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretRemoval {
    /// The declaration was removed.
    Removed,
    /// No such declaration exists.
    NotDeclared,
    /// A value is set; it must be cleared before removal.
    ValueSet,
    /// The declaration is still referenced by the listed fields.
    Referenced(Vec<String>),
}

fn secret_error(error: &crate::config::secrets::SecretStoreError) -> StoreError {
    StoreError::Persistence {
        store: "secret_store",
        detail: error.to_string(),
    }
}

fn config_to_store(error: &ConfigError) -> StoreError {
    StoreError::Persistence {
        store: "config",
        detail: error.to_string(),
    }
}

fn source_kind_from_str(kind: &str) -> SourceKind {
    match kind {
        "bundle" => SourceKind::Bundle,
        _ => SourceKind::Yaml,
    }
}

/// Legacy per-entity configuration tables that must not coexist with the opaque
/// snapshot store. Their presence with data and no active snapshot indicates a
/// pre-cutover database that requires an explicit reset.
const LEGACY_CONFIG_TABLES: &[&str] = &[
    "connections",
    "agents",
    "gateways",
    "kernel_configs",
    "git_agent_config",
    "secrets",
    "skills",
    "skill_versions",
];

/// Refuse to start against a pre-cutover database that still holds legacy
/// per-entity configuration rows but has no active snapshot.
fn guard_against_legacy_tables(path: &str, has_active_snapshot: bool) -> Result<(), StoreError> {
    use rusqlite::OptionalExtension;

    if has_active_snapshot {
        return Ok(());
    }
    let connection = rusqlite::Connection::open(path).map_err(|error| StoreError::Persistence {
        store: "config",
        detail: error.to_string(),
    })?;
    let mut populated = Vec::new();
    for table in LEGACY_CONFIG_TABLES {
        let exists: bool = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |_row| Ok(true),
            )
            .optional()
            .map_err(|error| StoreError::Persistence {
                store: "config",
                detail: error.to_string(),
            })?
            .unwrap_or(false);
        if !exists {
            continue;
        }
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .map_err(|error| StoreError::Persistence {
                store: "config",
                detail: error.to_string(),
            })?;
        if count > 0 {
            populated.push((*table).to_owned());
        }
    }
    drop(connection);
    if populated.is_empty() {
        Ok(())
    } else {
        Err(StoreError::Persistence {
            store: "config",
            detail: format!(
                "database contains legacy configuration tables with data ({}) but no active \
                 configuration snapshot; this schema is not migrated. Reset the database (delete \
                 the file or drop these tables) and re-apply configuration via /config/apply",
                populated.join(", ")
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        error::Error,
        fs,
        path::{Path, PathBuf},
    };

    use super::{ConfigState, SourceKind};

    const SOURCE: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: be helpful
";

    fn sqlite_test_path() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sqlite-tests");
        fs::create_dir_all(&directory)?;
        Ok(directory.join(format!("{}.db", uuid::Uuid::now_v7().simple())))
    }

    fn cleanup(path: &Path) {
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
    fn snapshot_source_bytes_persist_across_reopen() -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = sqlite_test_path()?;
        let db = path.to_string_lossy().into_owned();
        let env = BTreeMap::new();

        let (generation, source_sha, semantic_sha) = {
            let state = ConfigState::open(Some(&db), &env)?;
            let outcome = state.apply(SOURCE.as_bytes(), SourceKind::Yaml, &BTreeSet::new())?;
            (
                outcome.snapshot.generation,
                outcome.snapshot.source_sha256,
                outcome.snapshot.semantic_sha256,
            )
        };

        {
            let reopened = ConfigState::open(Some(&db), &env)?;
            // Exact source bytes survive the reopen byte-for-byte.
            assert_eq!(reopened.source_bytes()?, SOURCE.as_bytes());
            let snapshot = reopened
                .active_snapshot()?
                .ok_or("expected an active snapshot after reopen")?;
            assert_eq!(snapshot.generation, generation);
            assert_eq!(snapshot.source_sha256, source_sha);
            assert_eq!(snapshot.semantic_sha256, semantic_sha);
            // The active document is rehydrated from the persisted source.
            assert_eq!(reopened.active().spec.agents.len(), 1);
        }

        cleanup(&path);
        Ok(())
    }

    #[test]
    fn legacy_config_table_with_data_refuses_startup() -> Result<(), Box<dyn Error + Send + Sync>> {
        let path = sqlite_test_path()?;
        let db = path.to_string_lossy().into_owned();
        {
            let connection = rusqlite::Connection::open(&db)?;
            connection.execute_batch(
                "CREATE TABLE connections (id TEXT PRIMARY KEY, name TEXT NOT NULL);
                 INSERT INTO connections(id, name) VALUES('c1', 'legacy');",
            )?;
        }
        let result = ConfigState::open(Some(&db), &BTreeMap::new());
        assert!(result.is_err(), "legacy tables with data must fail startup");
        if let Err(error) = result {
            let message = error.to_string();
            assert!(
                message.contains("legacy configuration tables"),
                "unexpected error message: {message}"
            );
        }
        cleanup(&path);
        Ok(())
    }

    #[test]
    fn reapplying_identical_source_is_a_noop_generation() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let state = ConfigState::in_memory()?;
        let first = state.apply(SOURCE.as_bytes(), SourceKind::Yaml, &BTreeSet::new())?;
        let second = state.apply(SOURCE.as_bytes(), SourceKind::Yaml, &BTreeSet::new())?;
        // Identical source re-activates the same generation rather than growing.
        assert_eq!(first.snapshot.generation, second.snapshot.generation);
        assert_eq!(
            first.snapshot.semantic_sha256,
            second.snapshot.semantic_sha256
        );
        Ok(())
    }
}
