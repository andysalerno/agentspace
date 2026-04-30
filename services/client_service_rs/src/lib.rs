use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    num::ParseIntError,
    time::Duration,
};

use axum::Router;
use chrono::{DateTime, Utc};
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use uuid::Uuid;

use crate::{
    agent_host::AgentHostClient,
    store::{
        InMemoryAgentStore, InMemoryConnectionStore, InMemoryGatewayStore,
        InMemoryKernelConfigStore, InMemorySessionStore,
    },
};

pub mod agent_host;
pub mod api;
pub mod errors;
pub mod models;
pub mod store;

pub(crate) const ENV_PREFIX: &str = "CLIENT_SERVICE_";
const DEFAULT_BIND_HOST: &str = "0.0.0.0";
const DEFAULT_BIND_PORT: u16 = 8002;
const DEFAULT_AGENT_HOST_BASE_URL: &str = "http://127.0.0.1:8001";
const DEFAULT_AGENT_HOST_TIMEOUT_SECONDS: u64 = 60;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    bind_host: String,
    bind_port: u16,
    agent_host_base_url: String,
    pub(crate) client_service_env: BTreeMap<String, String>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_host =
            env::var("CLIENT_SERVICE_HOST").unwrap_or_else(|_| DEFAULT_BIND_HOST.to_owned());
        let bind_port = parse_port()?;
        let agent_host_base_url = env::var("CLIENT_SERVICE_AGENT_HOST_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_AGENT_HOST_BASE_URL.to_owned());
        let client_service_env = env::vars()
            .filter(|(key, _value)| key.starts_with(ENV_PREFIX))
            .collect();

        Ok(Self {
            bind_host,
            bind_port,
            agent_host_base_url,
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
            client_service_env,
        }
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
}

#[derive(Debug)]
pub enum ConfigError {
    InvalidPort { raw: String, source: ParseIntError },
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
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidPort { source, .. } => Some(source),
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: AppConfig,
    pub(crate) http_client: reqwest::Client,
    pub(crate) agent_host: AgentHostClient,
    pub(crate) agents: InMemoryAgentStore,
    pub(crate) kernel_configs: InMemoryKernelConfigStore,
    pub(crate) connections: InMemoryConnectionStore,
    pub(crate) gateways: InMemoryGatewayStore,
    pub(crate) sessions: InMemorySessionStore,
    pub(crate) instance_id: Uuid,
    pub(crate) started_at: DateTime<Utc>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let agent_host = AgentHostClient::new(
            config.agent_host_base_url(),
            Duration::from_secs(DEFAULT_AGENT_HOST_TIMEOUT_SECONDS),
        )?;
        Ok(Self::with_agent_host(config, agent_host))
    }

    #[must_use]
    pub fn with_agent_host(config: AppConfig, agent_host: AgentHostClient) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
            agent_host,
            agents: InMemoryAgentStore::new(),
            kernel_configs: InMemoryKernelConfigStore::new(),
            connections: InMemoryConnectionStore::new(),
            gateways: InMemoryGatewayStore::new(),
            sessions: InMemorySessionStore::new(),
            instance_id: Uuid::now_v7(),
            started_at: Utc::now(),
        }
    }
}

pub fn build_router(state: AppState) -> Router {
    api::router()
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

fn parse_port() -> Result<u16, ConfigError> {
    let raw = env::var("CLIENT_SERVICE_PORT").unwrap_or_else(|_| DEFAULT_BIND_PORT.to_string());
    raw.parse()
        .map_err(|source| ConfigError::InvalidPort { raw, source })
}
