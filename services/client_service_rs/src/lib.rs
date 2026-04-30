use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    num::ParseIntError,
};

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::Serialize;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use uuid::Uuid;

pub mod agent_host;
pub mod api;
pub mod errors;
pub mod models;
pub mod store;

const ENV_PREFIX: &str = "CLIENT_SERVICE_";
const DEFAULT_BIND_HOST: &str = "0.0.0.0";
const DEFAULT_BIND_PORT: u16 = 8002;
const DEFAULT_AGENT_HOST_BASE_URL: &str = "http://127.0.0.1:8001";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    bind_host: String,
    bind_port: u16,
    agent_host_base_url: String,
    client_service_env: BTreeMap<String, String>,
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
    config: AppConfig,
    http_client: reqwest::Client,
    instance_id: Uuid,
    started_at: DateTime<Utc>,
}

impl AppState {
    #[must_use]
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
            instance_id: Uuid::now_v7(),
            started_at: Utc::now(),
        }
    }
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/info", get(info))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

fn parse_port() -> Result<u16, ConfigError> {
    let raw = env::var("CLIENT_SERVICE_PORT").unwrap_or_else(|_| DEFAULT_BIND_PORT.to_string());
    raw.parse()
        .map_err(|source| ConfigError::InvalidPort { raw, source })
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn info(State(state): State<AppState>) -> Json<InfoResponse> {
    let _client = state.http_client.clone();

    Json(InfoResponse {
        client_service: ClientServiceInfo {
            service: "client_service",
            title: "Client Service",
            version: env!("CARGO_PKG_VERSION"),
            env_prefix: ENV_PREFIX,
            env: state.config.client_service_env,
            instance_id: state.instance_id,
            started_at: state.started_at,
        },
        agent_host: AgentHostInfo {
            service: "agent_host",
            base_url: state.config.agent_host_base_url,
        },
    })
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct InfoResponse {
    client_service: ClientServiceInfo,
    agent_host: AgentHostInfo,
}

#[derive(Debug, Serialize)]
struct ClientServiceInfo {
    service: &'static str,
    title: &'static str,
    version: &'static str,
    env_prefix: &'static str,
    env: BTreeMap<String, String>,
    instance_id: Uuid,
    started_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct AgentHostInfo {
    service: &'static str,
    base_url: String,
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, error::Error};

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{AppConfig, AppState, build_router};

    fn test_router() -> Router {
        let mut env = BTreeMap::new();
        env.insert("CLIENT_SERVICE_TEST".to_owned(), "enabled".to_owned());

        let config = AppConfig::new("127.0.0.1", 0, "http://agent-host.example.test:8001", env);

        build_router(AppState::new(config))
    }

    async fn get_json(path: &str) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        let response = test_router()
            .oneshot(Request::builder().uri(path).body(Body::empty())?)
            .await?;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        let value = serde_json::from_slice(&body)?;

        Ok((status, value))
    }

    #[tokio::test]
    async fn healthz_returns_ok() -> Result<(), Box<dyn Error + Send + Sync>> {
        let (status, value) = get_json("/healthz").await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(value, json!({ "status": "ok" }));

        Ok(())
    }

    #[tokio::test]
    async fn info_returns_basic_service_shape() -> Result<(), Box<dyn Error + Send + Sync>> {
        let (status, value) = get_json("/info").await?;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["client_service"]["service"], "client_service");
        assert_eq!(value["client_service"]["title"], "Client Service");
        assert_eq!(
            value["client_service"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(value["client_service"]["env_prefix"], "CLIENT_SERVICE_");
        assert_eq!(
            value["client_service"]["env"],
            json!({ "CLIENT_SERVICE_TEST": "enabled" })
        );
        assert!(matches!(
            value["client_service"]["instance_id"].as_str(),
            Some(raw) if Uuid::parse_str(raw).is_ok()
        ));
        assert!(value["client_service"]["started_at"].as_str().is_some());
        assert_eq!(value["agent_host"]["service"], "agent_host");
        assert_eq!(
            value["agent_host"]["base_url"],
            "http://agent-host.example.test:8001"
        );

        Ok(())
    }
}
