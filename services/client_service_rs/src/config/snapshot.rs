use std::{path::Path, sync::Mutex};

use rusqlite::{Connection, OptionalExtension};

use crate::{config::canonical::sha256_hex, errors::StoreError, models::utc_now};

const STORE: &str = "config_snapshots";

/// The exact source form retained for a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    Yaml,
    Bundle,
}

impl SourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Yaml => "yaml",
            Self::Bundle => "bundle",
        }
    }
}

/// An opaque, transactional snapshot envelope.
///
/// The database has no columns for agents, connections, gateways, kernel
/// fields, skills, or secret declarations, so it cannot drift from the YAML at
/// the field level.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub generation: i64,
    pub source_kind: String,
    pub source_bytes: Vec<u8>,
    pub source_sha256: String,
    pub semantic_sha256: String,
    pub created_at: String,
}

enum Backend {
    InMemory(Mutex<InMemoryState>),
    Sqlite(Mutex<Connection>),
}

#[derive(Default)]
struct InMemoryState {
    snapshots: Vec<Snapshot>,
    active: Option<i64>,
}

/// The opaque snapshot store. `SQLite` is used only as a transactional envelope.
pub struct SnapshotStore {
    backend: Backend,
}

impl SnapshotStore {
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            backend: Backend::InMemory(Mutex::new(InMemoryState::default())),
        }
    }

    /// Open (and create if needed) the snapshot envelope in the given `SQLite`
    /// database file.
    ///
    /// # Errors
    /// Returns [`StoreError::Persistence`] if the database cannot be opened or
    /// initialized.
    pub fn sqlite(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path.as_ref()).map_err(|e| persistence(&e))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS config_snapshots (
                    generation      INTEGER PRIMARY KEY,
                    source_kind     TEXT NOT NULL,
                    source_bytes    BLOB NOT NULL,
                    source_sha256   TEXT NOT NULL UNIQUE,
                    semantic_sha256 TEXT NOT NULL,
                    created_at      TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS active_config (
                    id         INTEGER PRIMARY KEY CHECK (id = 1),
                    generation INTEGER NOT NULL REFERENCES config_snapshots(generation)
                );",
            )
            .map_err(|e| persistence(&e))?;
        Ok(Self {
            backend: Backend::Sqlite(Mutex::new(connection)),
        })
    }

    /// Return the active snapshot, if any.
    ///
    /// # Errors
    /// Returns [`StoreError`] on lock or persistence failure.
    pub fn active(&self) -> Result<Option<Snapshot>, StoreError> {
        match &self.backend {
            Backend::InMemory(state) => {
                let state = lock(state)?;
                Ok(state
                    .active
                    .and_then(|generation| find(&state.snapshots, generation).cloned()))
            }
            Backend::Sqlite(connection) => {
                let connection = lock(connection)?;
                let generation: Option<i64> = connection
                    .query_row(
                        "SELECT generation FROM active_config WHERE id = 1",
                        [],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| persistence(&error))?;
                let result = generation.map_or(Ok(None), |generation| {
                    read_snapshot(&connection, generation)
                });
                drop(connection);
                result
            }
        }
    }

    /// Insert a new snapshot and switch the active pointer to it. If a snapshot
    /// with the same `source_sha256` already exists, its generation is
    /// re-activated instead of violating the unique-hash constraint.
    ///
    /// # Errors
    /// Returns [`StoreError`] on lock or persistence failure.
    pub fn insert_and_activate(
        &self,
        source_kind: SourceKind,
        source_bytes: Vec<u8>,
        semantic_sha256: String,
    ) -> Result<Snapshot, StoreError> {
        let source_sha256 = sha256_hex(&source_bytes);
        match &self.backend {
            Backend::InMemory(state) => {
                let mut state = lock(state)?;
                if let Some(existing) = state
                    .snapshots
                    .iter()
                    .find(|snapshot| snapshot.source_sha256 == source_sha256)
                    .cloned()
                {
                    state.active = Some(existing.generation);
                    return Ok(existing);
                }
                let generation = state
                    .snapshots
                    .iter()
                    .map(|snapshot| snapshot.generation)
                    .max()
                    .unwrap_or(0)
                    + 1;
                let snapshot = Snapshot {
                    generation,
                    source_kind: source_kind.as_str().to_owned(),
                    source_bytes,
                    source_sha256,
                    semantic_sha256,
                    created_at: utc_now(),
                };
                state.snapshots.push(snapshot.clone());
                state.active = Some(generation);
                drop(state);
                Ok(snapshot)
            }
            Backend::Sqlite(connection) => {
                let mut connection = lock(connection)?;
                let transaction = connection.transaction().map_err(|e| persistence(&e))?;
                if let Some(existing_generation) = transaction
                    .query_row(
                        "SELECT generation FROM config_snapshots WHERE source_sha256 = ?1",
                        [&source_sha256],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(|e| persistence(&e))?
                {
                    set_active(&transaction, existing_generation)?;
                    let snapshot = read_snapshot(&transaction, existing_generation)?;
                    transaction.commit().map_err(|e| persistence(&e))?;
                    drop(connection);
                    return snapshot.ok_or_else(|| StoreError::Persistence {
                        store: STORE,
                        detail: "active snapshot vanished".to_owned(),
                    });
                }
                let next_generation: i64 = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(generation), 0) + 1 FROM config_snapshots",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|e| persistence(&e))?;
                let created_at = utc_now();
                transaction
                    .execute(
                        "INSERT INTO config_snapshots(generation, source_kind, source_bytes, source_sha256, semantic_sha256, created_at)
                         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            next_generation,
                            source_kind.as_str(),
                            source_bytes,
                            source_sha256,
                            semantic_sha256,
                            created_at
                        ],
                    )
                    .map_err(|e| persistence(&e))?;
                set_active(&transaction, next_generation)?;
                transaction.commit().map_err(|e| persistence(&e))?;
                drop(connection);
                Ok(Snapshot {
                    generation: next_generation,
                    source_kind: source_kind.as_str().to_owned(),
                    source_bytes,
                    source_sha256,
                    semantic_sha256,
                    created_at,
                })
            }
        }
    }
}

fn set_active(connection: &Connection, generation: i64) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO active_config(id, generation) VALUES(1, ?1)
             ON CONFLICT(id) DO UPDATE SET generation = ?1",
            [generation],
        )
        .map_err(|e| persistence(&e))?;
    Ok(())
}

fn read_snapshot(connection: &Connection, generation: i64) -> Result<Option<Snapshot>, StoreError> {
    connection
        .query_row(
            "SELECT generation, source_kind, source_bytes, source_sha256, semantic_sha256, created_at
             FROM config_snapshots WHERE generation = ?1",
            [generation],
            |row| {
                Ok(Snapshot {
                    generation: row.get(0)?,
                    source_kind: row.get(1)?,
                    source_bytes: row.get(2)?,
                    source_sha256: row.get(3)?,
                    semantic_sha256: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|e| persistence(&e))
}

fn find(snapshots: &[Snapshot], generation: i64) -> Option<&Snapshot> {
    snapshots
        .iter()
        .find(|snapshot| snapshot.generation == generation)
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, StoreError> {
    mutex
        .lock()
        .map_err(|_| StoreError::LockPoisoned { store: STORE })
}

fn persistence(error: &rusqlite::Error) -> StoreError {
    StoreError::Persistence {
        store: STORE,
        detail: error.to_string(),
    }
}
