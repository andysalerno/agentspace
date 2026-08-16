use std::{
    collections::BTreeMap,
    convert::Infallible,
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    num::ParseIntError,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::MatchedPath,
    http::{Request, Response},
};
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tower_http::{
    classify::ServerErrorsFailureClass,
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing::Span;
use uuid::Uuid;

use crate::{
    agent_host::AgentHostClient,
    config::state::ConfigState,
    memory::MemoryProxyClient,
    store::{
        AgentStore, ConnectionStore, GatewayStore, KernelConfigStore, SessionStore, StoreSet,
        WorkspaceStore,
    },
};

pub mod agent_host;
pub mod api;
pub mod config;
pub mod errors;
pub mod memory;
pub mod models;
pub mod store;

pub(crate) const ENV_PREFIX: &str = "CLIENT_SERVICE_";
pub(crate) type StreamItem = Result<Vec<u8>, Infallible>;
pub(crate) type StreamSender = mpsc::Sender<StreamItem>;
const DEFAULT_BIND_HOST: &str = "0.0.0.0";
const DEFAULT_BIND_PORT: u16 = 8002;
const DEFAULT_AGENT_HOST_BASE_URL: &str = "http://127.0.0.1:8001";
const DEFAULT_AGENT_HOST_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_MEMORY_CONTAINER_BASE_URL: &str = "http://memory:8005";
const DEFAULT_MEMORY_LOCAL_BASE_URL: &str = "http://127.0.0.1:8005";
const DEFAULT_MEMORY_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_CONNECTION_MODELS_TIMEOUT_SECONDS: u64 = 15;
/// Browser origins allowed for cross-origin (CORS) requests by default. These
/// are the local `WebUI` dev/prod origins; production deployments override the
/// list via `CLIENT_SERVICE_CORS_ALLOWED_ORIGINS`.
const DEFAULT_CORS_ALLOWED_ORIGINS: &[&str] = &[
    "http://localhost:8003",
    "http://127.0.0.1:8003",
    "http://localhost:5173",
    "http://127.0.0.1:5173",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    bind_host: String,
    bind_port: u16,
    agent_host_base_url: String,
    memory_base_url: String,
    memory_timeout: Duration,
    connection_models_timeout: Duration,
    cors_allowed_origins: Vec<String>,
    pub(crate) client_service_env: BTreeMap<String, String>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_host =
            env::var("CLIENT_SERVICE_HOST").unwrap_or_else(|_| DEFAULT_BIND_HOST.to_owned());
        let bind_port = parse_port()?;
        let agent_host_base_url = env::var("CLIENT_SERVICE_AGENT_HOST_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_AGENT_HOST_BASE_URL.to_owned());
        let memory_base_url = env::var("CLIENT_SERVICE_MEMORY_BASE_URL")
            .unwrap_or_else(|_| default_memory_base_url());
        let memory_timeout = parse_duration_seconds_env(
            "CLIENT_SERVICE_MEMORY_TIMEOUT",
            DEFAULT_MEMORY_TIMEOUT_SECONDS,
        )?;
        let connection_models_timeout = parse_duration_seconds_env(
            "CLIENT_SERVICE_CONNECTION_MODELS_TIMEOUT",
            DEFAULT_CONNECTION_MODELS_TIMEOUT_SECONDS,
        )?;
        let client_service_env = env::vars()
            .filter(|(key, _value)| key.starts_with(ENV_PREFIX))
            .collect();

        let cors_allowed_origins = parse_cors_allowed_origins(
            env::var("CLIENT_SERVICE_CORS_ALLOWED_ORIGINS")
                .ok()
                .as_deref(),
        );

        Ok(Self {
            bind_host,
            bind_port,
            agent_host_base_url,
            memory_base_url,
            memory_timeout,
            connection_models_timeout,
            cors_allowed_origins,
            client_service_env,
        })
    }

    #[must_use]
    pub fn new(
        bind_host: impl Into<String>,
        bind_port: u16,
        agent_host_base_url: impl Into<String>,
        client_service_env: BTreeMap<String, String>,
    ) -> Self {
        Self {
            bind_host: bind_host.into(),
            bind_port,
            agent_host_base_url: agent_host_base_url.into(),
            memory_base_url: default_memory_base_url(),
            memory_timeout: Duration::from_secs(DEFAULT_MEMORY_TIMEOUT_SECONDS),
            connection_models_timeout: Duration::from_secs(
                DEFAULT_CONNECTION_MODELS_TIMEOUT_SECONDS,
            ),
            cors_allowed_origins: parse_cors_allowed_origins(None),
            client_service_env,
        }
    }

    #[must_use]
    pub fn with_memory_base_url(mut self, memory_base_url: impl Into<String>) -> Self {
        self.memory_base_url = memory_base_url.into();
        self
    }

    #[must_use]
    pub const fn with_memory_timeout(mut self, timeout: Duration) -> Self {
        self.memory_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn with_connection_models_timeout(mut self, timeout: Duration) -> Self {
        self.connection_models_timeout = timeout;
        self
    }

    #[must_use]
    pub fn bind_host(&self) -> &str {
        &self.bind_host
    }

    #[must_use]
    pub const fn bind_port(&self) -> u16 {
        self.bind_port
    }

    #[must_use]
    pub fn agent_host_base_url(&self) -> &str {
        &self.agent_host_base_url
    }

    #[must_use]
    pub fn memory_base_url(&self) -> &str {
        &self.memory_base_url
    }

    #[must_use]
    pub const fn memory_timeout(&self) -> Duration {
        self.memory_timeout
    }

    #[must_use]
    pub const fn connection_models_timeout(&self) -> Duration {
        self.connection_models_timeout
    }

    #[must_use]
    pub fn cors_allowed_origins(&self) -> &[String] {
        &self.cors_allowed_origins
    }

    #[must_use]
    pub fn with_cors_allowed_origins<I, S>(mut self, origins: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.cors_allowed_origins = origins.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn db_path(&self) -> Option<&str> {
        self.client_service_env
            .get("CLIENT_SERVICE_DB_PATH")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    /// Return the collected `CLIENT_SERVICE_`-prefixed environment with the
    /// values of sensitive variables replaced by a redaction placeholder.
    ///
    /// This is what `/info` and any diagnostic surface must expose: secret
    /// material (the master encryption key and any variable whose name matches
    /// a sensitive heuristic) must never leave the process. Non-sensitive keys keep their real values so `/info` remains
    /// useful for debugging deployment wiring.
    #[must_use]
    pub fn redacted_env(&self) -> BTreeMap<String, String> {
        self.client_service_env
            .iter()
            .map(|(key, value)| {
                let value = if is_sensitive_env_key(key) {
                    REDACTED_ENV_PLACEHOLDER.to_owned()
                } else {
                    value.clone()
                };
                (key.clone(), value)
            })
            .collect()
    }
}

/// Placeholder substituted for the value of any sensitive environment variable.
pub(crate) const REDACTED_ENV_PLACEHOLDER: &str = "***redacted***";

/// Environment variable names whose values are always sensitive and must be
/// redacted regardless of the heuristic below.
const SENSITIVE_ENV_KEYS: &[&str] = &["CLIENT_SERVICE_SECRET_KEY"];

/// Substrings that mark an environment variable name as sensitive. Matched
/// case-insensitively so, e.g., `CLIENT_SERVICE_OPENAI_API_KEY` is redacted.
const SENSITIVE_ENV_SUBSTRINGS: &[&str] = &["SECRET", "TOKEN", "PASSWORD", "KEY"];

/// Whether the value of the environment variable named `key` must be redacted.
#[must_use]
pub(crate) fn is_sensitive_env_key(key: &str) -> bool {
    if SENSITIVE_ENV_KEYS.contains(&key) {
        return true;
    }
    let upper = key.to_ascii_uppercase();
    SENSITIVE_ENV_SUBSTRINGS
        .iter()
        .any(|needle| upper.contains(needle))
}

#[derive(Debug)]
pub enum ConfigError {
    InvalidPort {
        raw: String,
        source: ParseIntError,
    },
    InvalidDuration {
        name: &'static str,
        raw: String,
        source: ParseIntError,
    },
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPort { raw, source } => {
                write!(
                    formatter,
                    "CLIENT_SERVICE_PORT must be a valid TCP port, got {raw:?}: {source}"
                )
            }
            Self::InvalidDuration { name, raw, source } => {
                write!(formatter, "{name} must be seconds, got {raw:?}: {source}")
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidPort { source, .. } | Self::InvalidDuration { source, .. } => Some(source),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: AppConfig,
    pub(crate) http_client: reqwest::Client,
    pub(crate) agent_host: AgentHostClient,
    pub(crate) memory: MemoryProxyClient,
    pub(crate) config_state: ConfigState,
    pub(crate) agents: AgentStore,
    pub(crate) kernel_configs: KernelConfigStore,
    pub(crate) connections: ConnectionStore,
    pub(crate) gateways: GatewayStore,
    pub(crate) workspaces: WorkspaceStore,
    pub(crate) sessions: SessionStore,
    pub(crate) active_turns: Arc<Mutex<BTreeMap<String, ActiveTurnRecord>>>,
    pub(crate) session_lifecycle: SessionLifecycleLocks,
    /// Serializes `/config/apply` end to end (validate → stage skills → commit
    /// → reconcile gateways) so two applies cannot interleave reconciliation.
    pub(crate) apply_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) instance_id: Uuid,
    pub(crate) started_at: DateTime<Utc>,
}

#[derive(Clone)]
pub(crate) struct ActiveTurnRecord {
    pub(crate) turn_id: String,
    pub(crate) user_message_id: String,
    pub(crate) assistant_message_id: String,
    pub(crate) stream: Option<Arc<Mutex<ActiveTurnStreamState>>>,
}

pub(crate) struct ActiveTurnStreamState {
    pub(crate) subscribers: Vec<StreamSender>,
    pub(crate) final_payload: Option<Vec<u8>>,
}

#[derive(Clone, Default)]
pub(crate) struct SessionLifecycleLocks {
    gate: Arc<tokio::sync::RwLock<()>>,
    sessions: Arc<tokio::sync::Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

pub(crate) struct SessionLifecycleGuard {
    _gate: tokio::sync::OwnedRwLockReadGuard<()>,
    _session: tokio::sync::OwnedMutexGuard<()>,
}

impl SessionLifecycleLocks {
    pub(crate) async fn lock(&self, session_id: &str) -> SessionLifecycleGuard {
        let gate = self.gate.clone().read_owned().await;
        let session = {
            let mut sessions = self.sessions.lock().await;
            sessions
                .entry(session_id.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        }
        .lock_owned()
        .await;
        SessionLifecycleGuard {
            _gate: gate,
            _session: session,
        }
    }

    pub(crate) async fn lock_cleanup(&self) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        self.gate.clone().write_owned().await
    }
}

impl AppState {
    pub fn new(config: AppConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let agent_host = AgentHostClient::new(
            config.agent_host_base_url(),
            Duration::from_secs(DEFAULT_AGENT_HOST_TIMEOUT_SECONDS),
        )?;
        let stores = config
            .db_path()
            .map_or_else(StoreSet::in_memory, StoreSet::sqlite)?;
        Ok(Self::with_clients_and_stores(config, agent_host, stores))
    }

    /// Build an app state with a custom agent-host client and in-memory stores.
    ///
    /// # Errors
    /// Returns an error if the in-memory stores cannot be initialized.
    pub fn with_agent_host(
        config: AppConfig,
        agent_host: AgentHostClient,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        Ok(Self::with_clients_and_stores(
            config,
            agent_host,
            StoreSet::in_memory()?,
        ))
    }

    #[must_use]
    pub fn with_clients_and_stores(
        config: AppConfig,
        agent_host: AgentHostClient,
        stores: StoreSet,
    ) -> Self {
        Self {
            memory: MemoryProxyClient::new(config.memory_base_url(), config.memory_timeout()),
            config,
            http_client: reqwest::Client::new(),
            agent_host,
            config_state: stores.config.clone(),
            agents: stores.agents,
            kernel_configs: stores.kernel_configs,
            connections: stores.connections,
            gateways: stores.gateways,
            workspaces: stores.workspaces,
            sessions: stores.sessions,
            active_turns: Arc::new(Mutex::new(BTreeMap::new())),
            session_lifecycle: SessionLifecycleLocks::default(),
            apply_lock: Arc::new(tokio::sync::Mutex::new(())),
            instance_id: Uuid::now_v7(),
            started_at: Utc::now(),
        }
    }
}

fn default_memory_base_url() -> String {
    if std::path::Path::new("/.dockerenv").exists() {
        DEFAULT_MEMORY_CONTAINER_BASE_URL.to_owned()
    } else {
        DEFAULT_MEMORY_LOCAL_BASE_URL.to_owned()
    }
}

/// Parse a comma-separated list of allowed CORS origins, falling back to the
/// default local `WebUI` origins when unset or empty. Blank entries are ignored.
fn parse_cors_allowed_origins(raw: Option<&str>) -> Vec<String> {
    let parsed: Vec<String> = raw
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if parsed.is_empty() {
        DEFAULT_CORS_ALLOWED_ORIGINS
            .iter()
            .map(|origin| (*origin).to_owned())
            .collect()
    } else {
        parsed
    }
}

/// Build a CORS layer that allows only the configured browser origins with an
/// explicit method and header allowlist. Requests without an `Origin` header
/// (CLI and service-to-service callers) are unaffected because CORS only
/// governs browser cross-origin access.
fn build_cors_layer(config: &AppConfig) -> CorsLayer {
    use axum::http::{HeaderName, Method, header};

    let origins: Vec<axum::http::HeaderValue> = config
        .cors_allowed_origins()
        .iter()
        .filter_map(|origin| origin.parse().ok())
        .collect();

    let methods = [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::PATCH,
        Method::OPTIONS,
    ];
    let headers: [HeaderName; 4] = [
        header::CONTENT_TYPE,
        header::IF_MATCH,
        header::AUTHORIZATION,
        header::ACCEPT,
    ];

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods(methods)
        .allow_headers(headers)
}

pub fn build_router(state: AppState) -> Router {
    let cors = build_cors_layer(&state.config);
    api::router().with_state(state).layer(cors).layer(
        TraceLayer::new_for_http()
            .make_span_with(|request: &Request<Body>| {
                let route = matched_route(request);
                tracing::info_span!(
                    "http_request",
                    method = %request.method(),
                    route = %route,
                    path = %request.uri().path(),
                    status = tracing::field::Empty,
                    latency_ms = tracing::field::Empty,
                )
            })
            .on_request(|request: &Request<Body>, span: &Span| {
                tracing::info!(
                    parent: span,
                    method = %request.method(),
                    route = %matched_route(request),
                    path = %request.uri().path(),
                    "http request started"
                );
            })
            .on_response(
                |response: &Response<Body>, latency: Duration, span: &Span| {
                    let latency_ms = duration_millis(latency);
                    span.record("status", response.status().as_u16());
                    span.record("latency_ms", latency_ms);
                    tracing::info!(
                        parent: span,
                        status = response.status().as_u16(),
                        latency_ms,
                        "http request completed"
                    );
                },
            )
            .on_failure(
                |failure_class: ServerErrorsFailureClass, latency: Duration, span: &Span| {
                    let latency_ms = duration_millis(latency);
                    tracing::warn!(
                        parent: span,
                        error_kind = ?failure_class,
                        latency_ms,
                        "http request failed"
                    );
                },
            ),
    )
}

fn matched_route<B>(request: &Request<B>) -> &str {
    request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| request.uri().path(), MatchedPath::as_str)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn parse_duration_seconds_env(
    name: &'static str,
    default_seconds: u64,
) -> Result<Duration, ConfigError> {
    let raw = env::var(name).unwrap_or_else(|_| default_seconds.to_string());
    let seconds = raw
        .parse::<u64>()
        .map_err(|source| ConfigError::InvalidDuration {
            name,
            raw: raw.clone(),
            source,
        })?;
    Ok(Duration::from_secs(seconds))
}

fn parse_port() -> Result<u16, ConfigError> {
    let raw = env::var("CLIENT_SERVICE_PORT").unwrap_or_else(|_| DEFAULT_BIND_PORT.to_string());
    raw.parse()
        .map_err(|source| ConfigError::InvalidPort { raw, source })
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, error::Error, fs, path::PathBuf};

    use tokio::time::{Duration, sleep, timeout};

    use crate::models::{AgentRecord, HarnessName};

    use super::{AppConfig, AppState, SessionLifecycleLocks};

    #[test]
    fn app_state_uses_sqlite_when_db_path_is_configured() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("sqlite-tests");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.db", uuid::Uuid::now_v7().simple()));
        let mut env = BTreeMap::new();
        env.insert(
            "CLIENT_SERVICE_DB_PATH".to_owned(),
            path.to_string_lossy().into_owned(),
        );

        let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", env.clone());
        {
            let state = AppState::new(config)?;
            state.agents.insert(&AgentRecord::new(
                "persisted",
                "Persisted",
                HarnessName::Acp,
                "prompt",
            ))?;
        }

        let reopened_config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", env);
        let reopened = AppState::new(reopened_config)?;
        assert!(reopened.agents.get("persisted")?.is_some());
        drop(reopened);

        let raw = path.to_string_lossy();
        for candidate in [
            path.clone(),
            PathBuf::from(format!("{raw}-wal")),
            PathBuf::from(format!("{raw}-shm")),
        ] {
            let _ignored = fs::remove_file(candidate);
        }
        Ok(())
    }

    #[tokio::test]
    async fn session_lifecycle_lock_serializes_same_session_and_cleanup() {
        let locks = SessionLifecycleLocks::default();
        let first = locks.lock("session").await;
        let same_session = {
            let locks = locks.clone();
            tokio::spawn(async move {
                let _guard = locks.lock("session").await;
            })
        };
        sleep(Duration::from_millis(20)).await;
        assert!(!same_session.is_finished());
        drop(first);
        timeout(Duration::from_secs(1), same_session)
            .await
            .unwrap_or_else(|_| panic!("same-session lifecycle lock stayed blocked"))
            .unwrap_or_else(|error| panic!("lifecycle task failed: {error}"));

        let session = locks.lock("session").await;
        let cleanup = {
            let locks = locks.clone();
            tokio::spawn(async move {
                let _guard = locks.lock_cleanup().await;
            })
        };
        sleep(Duration::from_millis(20)).await;
        assert!(!cleanup.is_finished());
        drop(session);
        timeout(Duration::from_secs(1), cleanup)
            .await
            .unwrap_or_else(|_| panic!("cleanup lifecycle lock stayed blocked"))
            .unwrap_or_else(|error| panic!("cleanup task failed: {error}"));
    }
}
