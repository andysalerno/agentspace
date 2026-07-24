use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Debug, Formatter},
    path::Path,
    sync::{Arc, Mutex},
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use rusqlite::Connection;
use zeroize::Zeroizing;

/// Environment variable that provides the base64-encoded 32-byte master key.
pub const MASTER_KEY_ENV: &str = "CLIENT_SERVICE_SECRET_KEY";

/// Errors raised by the secret store. Values are never included.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretStoreError {
    /// Encrypted values exist but no usable master key is available.
    MasterKeyUnavailable,
    /// The configured master key was not valid base64-encoded 32 bytes.
    InvalidMasterKey,
    /// A cryptographic operation failed.
    Crypto,
    /// A persistence error occurred.
    Persistence { detail: String },
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MasterKeyUnavailable => write!(
                formatter,
                "{MASTER_KEY_ENV} is required to decrypt stored secret values but is not set"
            ),
            Self::InvalidMasterKey => write!(
                formatter,
                "{MASTER_KEY_ENV} must be base64-encoded 32 bytes and must match the key used to \
                 encrypt existing stored secret values"
            ),
            Self::Crypto => write!(formatter, "secret encryption/decryption failed"),
            Self::Persistence { detail } => {
                write!(formatter, "secret store persistence error: {detail}")
            }
        }
    }
}

impl std::error::Error for SecretStoreError {}

/// A 32-byte AEAD master key. Never logged or serialized.
struct MasterKey {
    key: LessSafeKey,
    rng: SystemRandom,
}

impl MasterKey {
    fn from_bytes(bytes: &[u8; 32]) -> Result<Self, SecretStoreError> {
        let unbound =
            UnboundKey::new(&AES_256_GCM, bytes).map_err(|_| SecretStoreError::InvalidMasterKey)?;
        Ok(Self {
            key: LessSafeKey::new(unbound),
            rng: SystemRandom::new(),
        })
    }

    fn from_base64(encoded: &str) -> Result<Self, SecretStoreError> {
        let decoded = BASE64
            .decode(encoded.trim())
            .map_err(|_| SecretStoreError::InvalidMasterKey)?;
        let bytes: [u8; 32] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| SecretStoreError::InvalidMasterKey)?;
        let bytes = Zeroizing::new(bytes);
        Self::from_bytes(&bytes)
    }

    fn generate() -> Result<Self, SecretStoreError> {
        let rng = SystemRandom::new();
        let mut bytes = Zeroizing::new([0u8; 32]);
        rng.fill(bytes.as_mut())
            .map_err(|_| SecretStoreError::Crypto)?;
        Self::from_bytes(&bytes)
    }

    fn seal(&self, plaintext: &str) -> Result<Vec<u8>, SecretStoreError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce_bytes)
            .map_err(|_| SecretStoreError::Crypto)?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);
        let mut in_out = plaintext.as_bytes().to_vec();
        self.key
            .seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| SecretStoreError::Crypto)?;
        let mut sealed = Vec::with_capacity(NONCE_LEN + in_out.len());
        sealed.extend_from_slice(&nonce_bytes);
        sealed.extend_from_slice(&in_out);
        Ok(sealed)
    }

    fn open(&self, sealed: &[u8]) -> Result<String, SecretStoreError> {
        if sealed.len() < NONCE_LEN {
            return Err(SecretStoreError::Crypto);
        }
        let (nonce_bytes, ciphertext) = sealed.split_at(NONCE_LEN);
        let nonce =
            Nonce::try_assume_unique_for_key(nonce_bytes).map_err(|_| SecretStoreError::Crypto)?;
        let mut in_out = ciphertext.to_vec();
        let plaintext = self
            .key
            .open_in_place(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| SecretStoreError::Crypto)?;
        String::from_utf8(plaintext.to_vec()).map_err(|_| SecretStoreError::Crypto)
    }
}

impl Debug for MasterKey {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("MasterKey(<redacted>)")
    }
}

enum Backend {
    InMemory(Mutex<BTreeMap<String, Vec<u8>>>),
    Sqlite(Mutex<Connection>),
}

/// A write-only store for secret values, encrypted at rest.
///
/// Values are set/replaced/cleared only through explicit operations and are
/// never exposed by list, export, diff, error, or log output. Only the lazy
/// resolver reads a value, at the point an effective configuration is consumed.
pub struct SecretStore {
    backend: Backend,
    master_key: MasterKey,
    /// When false, the store is persistent but no master key was configured, so
    /// writing new ciphertext would be unrecoverable on the next startup.
    writable: bool,
}

impl Debug for SecretStore {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretStore")
            .field("values", &"<redacted>")
            .finish()
    }
}

impl SecretStore {
    /// Build a secret store, reading the master key from the environment.
    ///
    /// For the in-memory backend, an ephemeral key is generated when no master
    /// key is configured, so values are still protected in memory and never
    /// stored as plaintext.
    ///
    /// For the persistent (`SQLite`) backend, if any encrypted value already
    /// exists the master key is required and every stored ciphertext must
    /// decrypt with it; a missing or wrong key fails startup. When no ciphertext
    /// exists and no key is configured the store opens read-only for values so
    /// no unrecoverable ciphertext can be written.
    ///
    /// # Errors
    /// Returns [`SecretStoreError::MasterKeyUnavailable`] when persisted
    /// ciphertext exists but no key is configured,
    /// [`SecretStoreError::InvalidMasterKey`] if the configured key is malformed
    /// or cannot decrypt existing ciphertext, or a persistence error while
    /// opening `SQLite`.
    pub fn open(
        db_path: Option<&str>,
        env: &BTreeMap<String, String>,
    ) -> Result<Arc<Self>, SecretStoreError> {
        let configured = match env.get(MASTER_KEY_ENV) {
            Some(encoded) if !encoded.is_empty() => Some(MasterKey::from_base64(encoded)?),
            _ => None,
        };
        match db_path {
            None => {
                let master_key = match configured {
                    Some(key) => key,
                    None => MasterKey::generate()?,
                };
                Ok(Arc::new(Self {
                    backend: Backend::InMemory(Mutex::new(BTreeMap::new())),
                    master_key,
                    writable: true,
                }))
            }
            Some(path) => {
                let connection = Connection::open(Path::new(path)).map_err(|error| {
                    SecretStoreError::Persistence {
                        detail: error.to_string(),
                    }
                })?;
                connection
                    .execute_batch(
                        "CREATE TABLE IF NOT EXISTS secret_values (
                            name TEXT PRIMARY KEY,
                            ciphertext BLOB NOT NULL,
                            created_at TEXT NOT NULL
                        );",
                    )
                    .map_err(|error| SecretStoreError::Persistence {
                        detail: error.to_string(),
                    })?;
                let (master_key, writable) = if let Some(key) = configured {
                    verify_all_decrypt(&connection, &key)?;
                    (key, true)
                } else if ciphertext_count(&connection)? > 0 {
                    return Err(SecretStoreError::MasterKeyUnavailable);
                } else {
                    (MasterKey::generate()?, false)
                };
                Ok(Arc::new(Self {
                    backend: Backend::Sqlite(Mutex::new(connection)),
                    master_key,
                    writable,
                }))
            }
        }
    }

    /// Set or replace the value for a declaration. The declaration existence
    /// check is enforced by the caller against the config document.
    ///
    /// # Errors
    /// Returns [`SecretStoreError::MasterKeyUnavailable`] when the store is
    /// persistent but no master key is configured, or an error if encryption or
    /// persistence fails.
    pub fn set_value(&self, name: &str, value: &str) -> Result<(), SecretStoreError> {
        if !self.writable {
            return Err(SecretStoreError::MasterKeyUnavailable);
        }
        let sealed = self.master_key.seal(value)?;
        match &self.backend {
            Backend::InMemory(map) => {
                let mut map = map.lock().map_err(|_| SecretStoreError::Crypto)?;
                map.insert(name.to_owned(), sealed);
            }
            Backend::Sqlite(connection) => {
                let connection = connection.lock().map_err(|_| SecretStoreError::Crypto)?;
                connection
                    .execute(
                        "INSERT INTO secret_values(name, ciphertext, created_at)
                         VALUES(?1, ?2, ?3)
                         ON CONFLICT(name) DO UPDATE SET ciphertext = ?2, created_at = ?3",
                        rusqlite::params![name, sealed, crate::models::utc_now()],
                    )
                    .map_err(|error| SecretStoreError::Persistence {
                        detail: error.to_string(),
                    })?;
            }
        }
        Ok(())
    }

    /// Clear the value for a declaration. Returns whether a value existed.
    ///
    /// # Errors
    /// Returns an error if persistence fails.
    pub fn clear_value(&self, name: &str) -> Result<bool, SecretStoreError> {
        match &self.backend {
            Backend::InMemory(map) => {
                let mut map = map.lock().map_err(|_| SecretStoreError::Crypto)?;
                Ok(map.remove(name).is_some())
            }
            Backend::Sqlite(connection) => {
                let connection = connection.lock().map_err(|_| SecretStoreError::Crypto)?;
                let removed = connection
                    .execute("DELETE FROM secret_values WHERE name = ?1", [name])
                    .map_err(|error| SecretStoreError::Persistence {
                        detail: error.to_string(),
                    })?;
                drop(connection);
                Ok(removed > 0)
            }
        }
    }

    /// Return whether a value is set for the given declaration.
    ///
    /// # Errors
    /// Returns an error if persistence fails.
    pub fn is_set(&self, name: &str) -> Result<bool, SecretStoreError> {
        Ok(self.set_names()?.contains(name))
    }

    /// Return the set of declaration names that currently have a value.
    ///
    /// # Errors
    /// Returns an error if persistence fails.
    pub fn set_names(&self) -> Result<BTreeSet<String>, SecretStoreError> {
        match &self.backend {
            Backend::InMemory(map) => {
                let map = map.lock().map_err(|_| SecretStoreError::Crypto)?;
                Ok(map.keys().cloned().collect())
            }
            Backend::Sqlite(connection) => {
                let connection = connection.lock().map_err(|_| SecretStoreError::Crypto)?;
                let mut statement = connection
                    .prepare("SELECT name FROM secret_values")
                    .map_err(|error| SecretStoreError::Persistence {
                        detail: error.to_string(),
                    })?;
                let names = statement
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|error| SecretStoreError::Persistence {
                        detail: error.to_string(),
                    })?
                    .collect::<Result<BTreeSet<String>, _>>()
                    .map_err(|error| SecretStoreError::Persistence {
                        detail: error.to_string(),
                    })?;
                drop(statement);
                drop(connection);
                Ok(names)
            }
        }
    }

    /// Resolve a value. Only the lazy resolver should call this; the returned
    /// plaintext must never be logged or serialized.
    ///
    /// # Errors
    /// Returns an error if decryption or persistence fails.
    pub fn resolve(&self, name: &str) -> Result<Option<String>, SecretStoreError> {
        let sealed = match &self.backend {
            Backend::InMemory(map) => {
                let map = map.lock().map_err(|_| SecretStoreError::Crypto)?;
                map.get(name).cloned()
            }
            Backend::Sqlite(connection) => {
                let connection = connection.lock().map_err(|_| SecretStoreError::Crypto)?;
                connection
                    .query_row(
                        "SELECT ciphertext FROM secret_values WHERE name = ?1",
                        [name],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .map(Some)
                    .or_else(|error| match error {
                        rusqlite::Error::QueryReturnedNoRows => Ok(None),
                        other => Err(SecretStoreError::Persistence {
                            detail: other.to_string(),
                        }),
                    })?
            }
        };
        match sealed {
            Some(bytes) => Ok(Some(self.master_key.open(&bytes)?)),
            None => Ok(None),
        }
    }
}

fn ciphertext_count(connection: &Connection) -> Result<i64, SecretStoreError> {
    connection
        .query_row("SELECT COUNT(*) FROM secret_values", [], |row| row.get(0))
        .map_err(|error| SecretStoreError::Persistence {
            detail: error.to_string(),
        })
}

/// Verify that every stored ciphertext decrypts with the given key. A failure
/// indicates a wrong master key and must fail startup.
fn verify_all_decrypt(connection: &Connection, key: &MasterKey) -> Result<(), SecretStoreError> {
    let mut statement = connection
        .prepare("SELECT ciphertext FROM secret_values")
        .map_err(|error| SecretStoreError::Persistence {
            detail: error.to_string(),
        })?;
    let rows = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|error| SecretStoreError::Persistence {
            detail: error.to_string(),
        })?;
    for row in rows {
        let sealed = row.map_err(|error| SecretStoreError::Persistence {
            detail: error.to_string(),
        })?;
        if key.open(&sealed).is_err() {
            return Err(SecretStoreError::InvalidMasterKey);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        error::Error,
        fs,
        path::{Path, PathBuf},
    };

    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    use super::{MASTER_KEY_ENV, SecretStore, SecretStoreError};

    type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

    fn sqlite_test_path() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sqlite-tests");
        fs::create_dir_all(&directory)?;
        Ok(directory.join(format!("secrets-{}.db", uuid::Uuid::now_v7().simple())))
    }

    fn cleanup(path: &Path) {
        let raw = path.to_string_lossy().into_owned();
        for candidate in [
            path.to_path_buf(),
            PathBuf::from(format!("{raw}-wal")),
            PathBuf::from(format!("{raw}-shm")),
        ] {
            let _ignored = fs::remove_file(candidate);
        }
    }

    fn key_env(bytes: [u8; 32]) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert(MASTER_KEY_ENV.to_owned(), BASE64.encode(bytes));
        env
    }

    #[test]
    fn set_resolve_and_clear_round_trip() -> TestResult {
        let store = SecretStore::open(None, &BTreeMap::new())?;
        assert!(!store.is_set("OPENAI_API_KEY")?);
        store.set_value("OPENAI_API_KEY", "sk-secret")?;
        assert!(store.is_set("OPENAI_API_KEY")?);
        assert_eq!(
            store.resolve("OPENAI_API_KEY")?.as_deref(),
            Some("sk-secret")
        );
        assert!(store.clear_value("OPENAI_API_KEY")?);
        assert!(!store.is_set("OPENAI_API_KEY")?);
        assert_eq!(store.resolve("OPENAI_API_KEY")?, None);
        Ok(())
    }

    #[test]
    fn in_memory_does_not_store_plaintext() -> TestResult {
        let store = SecretStore::open(None, &BTreeMap::new())?;
        store.set_value("TOKEN", "plaintext-value")?;
        // The resolved value round-trips, proving it was encrypted, not stored raw.
        assert_eq!(store.resolve("TOKEN")?.as_deref(), Some("plaintext-value"));
        Ok(())
    }

    #[test]
    fn sqlite_value_persists_and_reopens_with_key() -> TestResult {
        let path = sqlite_test_path()?;
        let db = path.to_string_lossy().into_owned();
        let env = key_env([7u8; 32]);
        {
            let store = SecretStore::open(Some(&db), &env)?;
            store.set_value("OPENAI_API_KEY", "sk-persisted")?;
        }
        {
            let reopened = SecretStore::open(Some(&db), &env)?;
            assert!(reopened.is_set("OPENAI_API_KEY")?);
            assert_eq!(
                reopened.resolve("OPENAI_API_KEY")?.as_deref(),
                Some("sk-persisted")
            );
        }
        cleanup(&path);
        Ok(())
    }

    #[test]
    fn sqlite_reopen_with_wrong_key_fails() -> TestResult {
        let path = sqlite_test_path()?;
        let db = path.to_string_lossy().into_owned();
        {
            let store = SecretStore::open(Some(&db), &key_env([1u8; 32]))?;
            store.set_value("OPENAI_API_KEY", "sk-persisted")?;
        }
        let result = SecretStore::open(Some(&db), &key_env([2u8; 32]));
        assert!(matches!(result, Err(SecretStoreError::InvalidMasterKey)));
        cleanup(&path);
        Ok(())
    }

    #[test]
    fn sqlite_reopen_without_key_fails_when_ciphertext_exists() -> TestResult {
        let path = sqlite_test_path()?;
        let db = path.to_string_lossy().into_owned();
        {
            let store = SecretStore::open(Some(&db), &key_env([3u8; 32]))?;
            store.set_value("OPENAI_API_KEY", "sk-persisted")?;
        }
        let result = SecretStore::open(Some(&db), &BTreeMap::new());
        assert!(matches!(
            result,
            Err(SecretStoreError::MasterKeyUnavailable)
        ));
        cleanup(&path);
        Ok(())
    }

    #[test]
    fn sqlite_without_key_is_read_only_for_values() -> TestResult {
        let path = sqlite_test_path()?;
        let db = path.to_string_lossy().into_owned();
        let store = SecretStore::open(Some(&db), &BTreeMap::new())?;
        let result = store.set_value("OPENAI_API_KEY", "sk-secret");
        assert!(matches!(
            result,
            Err(SecretStoreError::MasterKeyUnavailable)
        ));
        cleanup(&path);
        Ok(())
    }
}
