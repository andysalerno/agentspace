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
use tower_http::{classify::ServerErrorsFailureClass, cors::CorsLayer, trace::TraceLayer};
use tracing::Span;
use uuid::Uuid;

use crate::{
    agent_host::AgentHostClient,
    git_agent::GitAgentClient,
    memory::MemoryProxyClient,
    models::DEFAULT_GIT_AGENT_DATA_VOLUME,
    store::{
        AgentStore, ConnectionStore, GatewayStore, GitAgentConfigStore, KernelConfigStore,
        SessionStore, StoreSet, WorkspaceStore,
    },
};

pub mod agent_host;
pub mod api;
pub mod errors;
pub mod git_agent;
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
const DEFAULT_GIT_AGENT_CONTAINER_BASE_URL: &str = "http://git-agent:8004";
const DEFAULT_GIT_AGENT_LOCAL_BASE_URL: &str = "http://127.0.0.1:8004";
const DEFAULT_GIT_AGENT_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_MEMORY_CONTAINER_BASE_URL: &str = "http://memory:8005";
const DEFAULT_MEMORY_LOCAL_BASE_URL: &str = "http://127.0.0.1:8005";
const DEFAULT_MEMORY_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_CONNECTION_MODELS_TIMEOUT_SECONDS: u64 = 15;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    bind_host: String,
    bind_port: u16,
    agent_host_base_url: String,
    git_agent_base_url: String,
    git_agent_data_volume_name: String,
    memory_base_url: String,
    memory_timeout: Duration,
    connection_models_timeout: Duration,
    pub(crate) client_service_env: BTreeMap<String, String>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_host =
            env::var("CLIENT_SERVICE_HOST").unwrap_or_else(|_| DEFAULT_BIND_HOST.to_owned());
        let bind_port = parse_port()?;
        let agent_host_base_url = env::var("CLIENT_SERVICE_AGENT_HOST_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_AGENT_HOST_BASE_URL.to_owned());
        let git_agent_base_url = env::var("CLIENT_SERVICE_GIT_AGENT_BASE_URL")
            .unwrap_or_else(|_| default_git_agent_base_url());
        let git_agent_data_volume_name = env::var("CLIENT_SERVICE_GIT_AGENT_DATA_VOLUME")
            .unwrap_or_else(|_| DEFAULT_GIT_AGENT_DATA_VOLUME.to_owned());
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

        Ok(Self {
            bind_host,
            bind_port,
            agent_host_base_url,
            git_agent_base_url,
            git_agent_data_volume_name,
            memory_base_url,
            memory_timeout,
            connection_models_timeout,
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
            git_agent_base_url: default_git_agent_base_url(),
            git_agent_data_volume_name: DEFAULT_GIT_AGENT_DATA_VOLUME.to_owned(),
            memory_base_url: default_memory_base_url(),
            memory_timeout: Duration::from_secs(DEFAULT_MEMORY_TIMEOUT_SECONDS),
            connection_models_timeout: Duration::from_secs(
                DEFAULT_CONNECTION_MODELS_TIMEOUT_SECONDS,
            ),
            client_service_env,
        }
    }

    #[must_use]
    pub fn with_git_agent_base_url(mut self, git_agent_base_url: impl Into<String>) -> Self {
        self.git_agent_base_url = git_agent_base_url.into();
        self
    }

    #[must_use]
    pub fn with_git_agent_data_volume_name(
        mut self,
        git_agent_data_volume_name: impl Into<String>,
    ) -> Self {
        self.git_agent_data_volume_name = git_agent_data_volume_name.into();
        self
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
    pub fn git_agent_base_url(&self) -> &str {
        &self.git_agent_base_url
    }

    #[must_use]
    pub fn git_agent_data_volume_name(&self) -> &str {
        &self.git_agent_data_volume_name
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
    pub fn db_path(&self) -> Option<&str> {
        self.client_service_env
            .get("CLIENT_SERVICE_DB_PATH")
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
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
    pub(crate) git_agent: GitAgentClient,
    pub(crate) memory: MemoryProxyClient,
    pub(crate) agents: AgentStore,
    pub(crate) git_agent_config: GitAgentConfigStore,
    pub(crate) kernel_configs: KernelConfigStore,
    pub(crate) connections: ConnectionStore,
    pub(crate) gateways: GatewayStore,
    pub(crate) workspaces: WorkspaceStore,
    pub(crate) sessions: SessionStore,
    pub(crate) active_turns: Arc<Mutex<BTreeMap<String, ActiveTurnRecord>>>,
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

impl AppState {
    pub fn new(config: AppConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let agent_host = AgentHostClient::new(
            config.agent_host_base_url(),
            Duration::from_secs(DEFAULT_AGENT_HOST_TIMEOUT_SECONDS),
        )?;
        let git_agent = GitAgentClient::new(
            config.git_agent_base_url(),
            Duration::from_secs(DEFAULT_GIT_AGENT_TIMEOUT_SECONDS),
        );
        let stores = config
            .db_path()
            .map_or_else(|| Ok(StoreSet::in_memory()), StoreSet::sqlite)?;
        Ok(Self::with_clients_and_stores(
            config, agent_host, git_agent, stores,
        ))
    }

    #[must_use]
    pub fn with_agent_host(config: AppConfig, agent_host: AgentHostClient) -> Self {
        let git_agent = GitAgentClient::new(
            config.git_agent_base_url(),
            Duration::from_secs(DEFAULT_GIT_AGENT_TIMEOUT_SECONDS),
        );
        Self::with_clients_and_stores(config, agent_host, git_agent, StoreSet::in_memory())
    }

    #[must_use]
    pub fn with_clients_and_stores(
        config: AppConfig,
        agent_host: AgentHostClient,
        git_agent: GitAgentClient,
        stores: StoreSet,
    ) -> Self {
        Self {
            memory: MemoryProxyClient::new(config.memory_base_url(), config.memory_timeout()),
            config,
            http_client: reqwest::Client::new(),
            agent_host,
            git_agent,
            agents: stores.agents,
            git_agent_config: stores.git_agent_config,
            kernel_configs: stores.kernel_configs,
            connections: stores.connections,
            gateways: stores.gateways,
            workspaces: stores.workspaces,
            sessions: stores.sessions,
            active_turns: Arc::new(Mutex::new(BTreeMap::new())),
            instance_id: Uuid::now_v7(),
            started_at: Utc::now(),
        }
    }
}

fn default_git_agent_base_url() -> String {
    if std::path::Path::new("/.dockerenv").exists() {
        DEFAULT_GIT_AGENT_CONTAINER_BASE_URL.to_owned()
    } else {
        DEFAULT_GIT_AGENT_LOCAL_BASE_URL.to_owned()
    }
}

fn default_memory_base_url() -> String {
    if std::path::Path::new("/.dockerenv").exists() {
        DEFAULT_MEMORY_CONTAINER_BASE_URL.to_owned()
    } else {
        DEFAULT_MEMORY_LOCAL_BASE_URL.to_owned()
    }
}

pub fn build_router(state: AppState) -> Router {
    api::router()
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(
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

    use crate::store::AgentStore;

    use super::{AppConfig, AppState};

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

        let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", env);
        let state = AppState::new(config)?;
        assert!(matches!(state.agents, AgentStore::Sqlite(_)));
        drop(state);

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
}
