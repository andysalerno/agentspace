use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    num::ParseIntError,
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::MatchedPath,
    http::{Request, Response},
};
use chrono::{DateTime, Utc};
use tower_http::{classify::ServerErrorsFailureClass, cors::CorsLayer, trace::TraceLayer};
use tracing::Span;
use uuid::Uuid;

pub mod api;
pub mod docker_runtime;
pub mod errors;
pub mod gateways;
pub mod models;
pub mod sessions;
pub mod skills;
pub mod terminal;

pub const ENV_PREFIX: &str = "AGENT_HOST_";
const DEFAULT_BIND_HOST: &str = "0.0.0.0";
const DEFAULT_BIND_PORT: u16 = 8001;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    bind_host: String,
    bind_port: u16,
    pub(crate) agent_host_env: BTreeMap<String, String>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_host =
            env::var("AGENT_HOST_HOST").unwrap_or_else(|_| DEFAULT_BIND_HOST.to_owned());
        let bind_port = parse_port()?;
        let agent_host_env = env::vars()
            .filter(|(key, _value)| key.starts_with(ENV_PREFIX))
            .collect();

        Ok(Self {
            bind_host,
            bind_port,
            agent_host_env,
        })
    }

    #[must_use]
    pub fn new(
        bind_host: impl Into<String>,
        bind_port: u16,
        agent_host_env: BTreeMap<String, String>,
    ) -> Self {
        Self {
            bind_host: bind_host.into(),
            bind_port,
            agent_host_env,
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
                    "AGENT_HOST_PORT must be a valid TCP port, got {raw:?}: {source}"
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
    pub(crate) instance_id: Uuid,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) sessions: sessions::SessionRegistry,
    pub(crate) docker_runtime: docker_runtime::DockerRuntime,
    pub(crate) skills: skills::SkillRegistry,
    pub(crate) gateways: gateways::GatewayRegistry,
}

impl AppState {
    #[must_use]
    pub fn new(config: AppConfig) -> Self {
        match Self::try_new(config) {
            Ok(state) => state,
            Err(error) => panic!("failed to initialize agent host state: {error}"),
        }
    }

    pub fn try_new(config: AppConfig) -> Result<Self, skills::SkillError> {
        let skills = skills::SkillRegistry::try_from_env()?;
        Ok(Self::with_skill_registry(config, skills))
    }

    pub fn try_with_skills_service(
        config: AppConfig,
        service: skills::SkillsService,
    ) -> Result<Self, skills::SkillError> {
        let skills = skills::SkillRegistry::from_synced_service(service)?;
        Ok(Self::with_skill_registry(config, skills))
    }

    #[must_use]
    pub fn with_skill_registry(config: AppConfig, skills: skills::SkillRegistry) -> Self {
        let sessions = sessions::SessionRegistry::with_skills(skills.clone());
        Self {
            config,
            instance_id: Uuid::now_v7(),
            started_at: Utc::now(),
            sessions,
            docker_runtime: docker_runtime::DockerRuntime::default(),
            skills,
            gateways: gateways::GatewayRegistry::default(),
        }
    }

    pub async fn shutdown(&self) {
        self.sessions.forget_all_sessions().await;
        self.gateways.destroy_all_gateways().await;
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

fn parse_port() -> Result<u16, ConfigError> {
    let raw = env::var("AGENT_HOST_PORT").unwrap_or_else(|_| DEFAULT_BIND_PORT.to_string());
    raw.parse()
        .map_err(|source| ConfigError::InvalidPort { raw, source })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{AppConfig, AppState};

    #[test]
    fn app_state_uses_configured_agent_host_environment() {
        let mut env = BTreeMap::new();
        env.insert("AGENT_HOST_EXAMPLE".to_owned(), "enabled".to_owned());

        let config = AppConfig::new("127.0.0.1", 0, env.clone());
        let state = AppState::new(config);

        assert_eq!(state.config.agent_host_env, env);
    }
}
