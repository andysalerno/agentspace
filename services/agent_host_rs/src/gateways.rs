use std::{
    collections::{BTreeMap, HashMap},
    env,
    error::Error,
    fmt::{self, Debug, Display, Formatter},
    sync::{Arc, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bollard::{
    Docker,
    container::LogOutput,
    errors::Error as BollardError,
    models::{ContainerCreateBody, HostConfig},
    query_parameters::{
        CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    },
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::Mutex, time::Instant};

use crate::{AppState, models::ServiceSummary};

const DEFAULT_GATEWAY_IMAGE: &str = "agentspace-gateway-gateway:latest";
const DEFAULT_DOCKER_NETWORK: &str = "agentspace-stack";
const DEFAULT_GATEWAY_BASE_URL_TEMPLATE: &str = "http://{container_name}:8000";
const CONTAINER_NAME_PLACEHOLDER: &str = "{container_name}";
const DEFAULT_GATEWAY_CLIENT_SERVICE_URL: &str = "http://client-service:8002";
const DEFAULT_GATEWAY_STARTUP_TIMEOUT_SECONDS: f64 = 60.0;
const GATEWAY_LOG_TAIL_LINES: usize = 200;
const READINESS_POLL_INTERVAL: Duration = Duration::from_secs(1);

const GATEWAY_ENTRYPOINT: [&str; 7] = [
    "/usr/local/bin/uv",
    "run",
    "--no-dev",
    "--package",
    "gateway-host",
    "-m",
    "gateway_host.api_main",
];

#[derive(Clone, Default)]
pub struct GatewayRegistry {
    host: GatewayHost,
}

impl GatewayRegistry {
    #[must_use]
    pub fn new(runtime: impl GatewayRuntime) -> Self {
        Self {
            host: GatewayHost::new(runtime),
        }
    }

    #[must_use]
    pub const fn summary(&self) -> ServiceSummary {
        ServiceSummary {
            status: "ready",
            detail: "gateway lifecycle routes are active",
        }
    }

    pub async fn create_gateway(
        &self,
        request: GatewayRuntimeCreateRequest,
    ) -> Result<GatewaySummary, GatewayError> {
        self.host.create_gateway(request).await
    }

    pub async fn destroy_gateway(&self, gateway_id: &str) -> Result<(), GatewayError> {
        self.host.destroy_gateway(gateway_id).await
    }

    pub async fn destroy_all_gateways(&self) {
        self.host.destroy_all_gateways().await;
    }

    pub async fn list_gateways(&self) -> Vec<GatewaySummary> {
        self.host.list_gateways().await
    }

    pub async fn get_gateway(&self, gateway_id: &str) -> Result<GatewaySummary, GatewayError> {
        self.host.get_gateway(gateway_id).await
    }

    pub async fn gateway_logs(&self, gateway_id: &str) -> Result<Vec<String>, GatewayError> {
        self.host.gateway_logs(gateway_id).await
    }
}

#[derive(Clone)]
pub struct GatewayHost {
    runtime: Arc<dyn GatewayRuntime>,
    records: Arc<Mutex<BTreeMap<String, GatewayRecord>>>,
}

impl Default for GatewayHost {
    fn default() -> Self {
        Self::new(DockerGatewayRuntime::default())
    }
}

impl GatewayHost {
    #[must_use]
    pub fn new(runtime: impl GatewayRuntime) -> Self {
        Self {
            runtime: Arc::new(runtime),
            records: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub async fn create_gateway(
        &self,
        request: GatewayRuntimeCreateRequest,
    ) -> Result<GatewaySummary, GatewayError> {
        validate_gateway_id(&request.gateway_id)?;

        let mut records = self.records.lock().await;
        if records.contains_key(&request.gateway_id) {
            return Err(GatewayError::AlreadyExists {
                gateway_id: request.gateway_id,
            });
        }

        let runtime_session = self.runtime.create_gateway(request.clone()).await?;
        let record = GatewayRecord {
            gateway_id: request.gateway_id,
            gateway_type: request.gateway_type,
            agent_id: request.agent_id,
            runtime_session,
            env: request.env,
        };
        records.insert(record.gateway_id.clone(), record.clone());
        drop(records);

        Ok(self.summary_with_status(&record).await)
    }

    pub async fn destroy_gateway(&self, gateway_id: &str) -> Result<(), GatewayError> {
        let mut records = self.records.lock().await;
        let record = records
            .remove(gateway_id)
            .ok_or_else(|| GatewayError::NotFound {
                gateway_id: gateway_id.to_owned(),
            })?;
        drop(records);

        self.runtime.destroy_gateway(&record.runtime_session).await
    }

    pub async fn destroy_all_gateways(&self) {
        let mut records_guard = self.records.lock().await;
        let records = std::mem::take(&mut *records_guard);
        drop(records_guard);

        for record in records.into_values() {
            if let Err(error) = self.runtime.destroy_gateway(&record.runtime_session).await {
                tracing::warn!(
                    gateway_id = %record.gateway_id,
                    error = %error,
                    "failed to destroy gateway"
                );
            }
        }
    }

    pub async fn list_gateways(&self) -> Vec<GatewaySummary> {
        let records_guard = self.records.lock().await;
        let records = records_guard.values().cloned().collect::<Vec<_>>();
        drop(records_guard);

        let mut summaries = Vec::with_capacity(records.len());
        for record in records {
            summaries.push(self.summary_with_status(&record).await);
        }
        summaries
    }

    pub async fn get_gateway(&self, gateway_id: &str) -> Result<GatewaySummary, GatewayError> {
        let record = self.require_gateway(gateway_id).await?;
        Ok(self.summary_with_status(&record).await)
    }

    pub async fn gateway_logs(&self, gateway_id: &str) -> Result<Vec<String>, GatewayError> {
        let record = self.require_gateway(gateway_id).await?;
        self.runtime.logs(&record.runtime_session).await
    }

    async fn require_gateway(&self, gateway_id: &str) -> Result<GatewayRecord, GatewayError> {
        let records = self.records.lock().await;
        records
            .get(gateway_id)
            .cloned()
            .ok_or_else(|| GatewayError::NotFound {
                gateway_id: gateway_id.to_owned(),
            })
    }

    async fn summary_with_status(&self, record: &GatewayRecord) -> GatewaySummary {
        let status = self
            .runtime
            .status(&record.runtime_session)
            .await
            .unwrap_or_else(|error| GatewayRuntimeStatus::error(error.to_string()));

        record.summary_with_status(status)
    }
}

#[derive(Clone, Debug)]
pub struct GatewayRecord {
    pub gateway_id: String,
    pub gateway_type: String,
    pub agent_id: String,
    pub runtime_session: GatewayRuntimeSession,
    pub env: BTreeMap<String, String>,
}

impl GatewayRecord {
    #[must_use]
    pub fn summary_with_status(&self, status: GatewayRuntimeStatus) -> GatewaySummary {
        GatewaySummary {
            gateway_id: self.gateway_id.clone(),
            gateway_type: self.gateway_type.clone(),
            agent_id: self.agent_id.clone(),
            status: status.status,
            last_error: status.last_error,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GatewayRuntimeCreateRequest {
    pub gateway_id: String,
    pub gateway_type: String,
    pub agent_id: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GatewaySummary {
    pub gateway_id: String,
    #[serde(rename = "type")]
    pub gateway_type: String,
    pub agent_id: String,
    pub status: GatewayStatus,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayRuntimeStatus {
    pub status: GatewayStatus,
    pub last_error: Option<String>,
}

impl GatewayRuntimeStatus {
    #[must_use]
    pub const fn running() -> Self {
        Self {
            status: GatewayStatus::Running,
            last_error: None,
        }
    }

    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: GatewayStatus::Error,
            last_error: Some(message.into()),
        }
    }

    fn from_response(response: GatewayStatusResponse) -> Self {
        Self {
            status: response.status,
            last_error: response.last_error,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum GatewayStatus {
    Running,
    Error,
    #[default]
    Unknown,
    Other(String),
}

impl GatewayStatus {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Error => "error",
            Self::Unknown => "unknown",
            Self::Other(status) => status,
        }
    }
}

impl Display for GatewayStatus {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for GatewayStatus {
    fn from(status: &str) -> Self {
        match status {
            "running" => Self::Running,
            "error" => Self::Error,
            "unknown" => Self::Unknown,
            other => Self::Other(other.to_owned()),
        }
    }
}

impl Serialize for GatewayStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for GatewayStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let status = String::deserialize(deserializer)?;
        Ok(Self::from(status.as_str()))
    }
}

#[derive(Debug, Deserialize)]
struct GatewayStatusResponse {
    #[serde(default)]
    status: GatewayStatus,
    #[serde(default)]
    last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GatewayRuntimeSession {
    Docker(DockerGatewaySession),
    Opaque(String),
}

impl GatewayRuntimeSession {
    #[must_use]
    pub fn opaque(value: impl Into<String>) -> Self {
        Self::Opaque(value.into())
    }
}

#[async_trait]
pub trait GatewayRuntime: Send + Sync + 'static {
    async fn create_gateway(
        &self,
        request: GatewayRuntimeCreateRequest,
    ) -> Result<GatewayRuntimeSession, GatewayError>;

    async fn destroy_gateway(&self, session: &GatewayRuntimeSession) -> Result<(), GatewayError>;

    async fn status(
        &self,
        session: &GatewayRuntimeSession,
    ) -> Result<GatewayRuntimeStatus, GatewayError>;

    async fn logs(&self, session: &GatewayRuntimeSession) -> Result<Vec<String>, GatewayError>;
}

#[derive(Clone, Debug)]
pub struct DockerGatewayRuntime<C = BollardDockerGatewayClient>
where
    C: DockerGatewayClient,
{
    config: DockerGatewayConfig,
    docker: C,
    http_client: reqwest::Client,
}

impl Default for DockerGatewayRuntime<BollardDockerGatewayClient> {
    fn default() -> Self {
        Self::new(
            DockerGatewayConfig::from_env(),
            BollardDockerGatewayClient::default(),
        )
    }
}

impl<C> DockerGatewayRuntime<C>
where
    C: DockerGatewayClient,
{
    #[must_use]
    pub fn new(config: DockerGatewayConfig, docker: C) -> Self {
        let http_client = match reqwest::Client::builder()
            .timeout(config.startup_timeout)
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to build gateway HTTP client with timeout; using default client"
                );
                reqwest::Client::new()
            }
        };

        Self {
            config,
            docker,
            http_client,
        }
    }

    fn container_spec(
        &self,
        request: &GatewayRuntimeCreateRequest,
        container_name: &str,
    ) -> DockerGatewayContainerSpec {
        let mut environment = request.env.clone();
        environment.insert("GATEWAY_ID".to_owned(), request.gateway_id.clone());
        environment.insert("GATEWAY_TYPE".to_owned(), request.gateway_type.clone());
        environment.insert("GATEWAY_AGENT_ID".to_owned(), request.agent_id.clone());
        environment.insert(
            "GATEWAY_CLIENT_SERVICE_BASE_URL".to_owned(),
            self.config.client_service_url.clone(),
        );

        let labels = BTreeMap::from([
            ("agentspace.role".to_owned(), "gateway".to_owned()),
            (
                "agentspace.gateway_id".to_owned(),
                request.gateway_id.clone(),
            ),
            (
                "agentspace.gateway_type".to_owned(),
                request.gateway_type.clone(),
            ),
        ]);

        DockerGatewayContainerSpec {
            name: container_name.to_owned(),
            image: self.config.gateway_image.clone(),
            network: self.config.network.clone(),
            entrypoint: GATEWAY_ENTRYPOINT.iter().map(ToString::to_string).collect(),
            environment,
            labels,
        }
    }

    async fn wait_until_ready(&self, base_url: &str) -> Result<(), GatewayError> {
        let deadline = Instant::now() + self.config.startup_timeout;
        let healthz_url = format!("{base_url}/healthz");

        loop {
            match self.http_client.get(&healthz_url).send().await {
                Ok(response) if response.status() == StatusCode::OK => return Ok(()),
                Ok(response) => {
                    tracing::debug!(
                        url = %healthz_url,
                        status = response.status().as_u16(),
                        "gateway health check not ready"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        url = %healthz_url,
                        error = %error,
                        "gateway health check failed"
                    );
                }
            }

            if Instant::now() >= deadline {
                return Err(GatewayError::Runtime {
                    message: format!("gateway container at {base_url} did not become ready"),
                });
            }

            tokio::time::sleep(READINESS_POLL_INTERVAL).await;
        }
    }

    fn docker_session(
        session: &GatewayRuntimeSession,
    ) -> Result<&DockerGatewaySession, GatewayError> {
        match session {
            GatewayRuntimeSession::Docker(handle) => Ok(handle),
            GatewayRuntimeSession::Opaque(_) => Err(GatewayError::Runtime {
                message: format!("unsupported gateway session handle: {session:?}"),
            }),
        }
    }
}

#[async_trait]
impl<C> GatewayRuntime for DockerGatewayRuntime<C>
where
    C: DockerGatewayClient,
{
    async fn create_gateway(
        &self,
        request: GatewayRuntimeCreateRequest,
    ) -> Result<GatewayRuntimeSession, GatewayError> {
        let container_name = format!("agentspace-gateway-{}", request.gateway_id);
        let base_url = self.config.base_url_for(&container_name);
        let spec = self.container_spec(&request, &container_name);

        tracing::info!(
            gateway_id = %request.gateway_id,
            gateway_type = %request.gateway_type,
            agent_id = %request.agent_id,
            container_name = %container_name,
            "creating gateway container"
        );

        self.docker.remove_container(&container_name).await?;
        self.docker.create_container(&spec).await?;
        self.wait_until_ready(&base_url).await?;

        Ok(GatewayRuntimeSession::Docker(DockerGatewaySession {
            container_name,
            base_url,
        }))
    }

    async fn destroy_gateway(&self, session: &GatewayRuntimeSession) -> Result<(), GatewayError> {
        let handle = Self::docker_session(session)?;
        self.docker.remove_container(&handle.container_name).await
    }

    async fn status(
        &self,
        session: &GatewayRuntimeSession,
    ) -> Result<GatewayRuntimeStatus, GatewayError> {
        let handle = Self::docker_session(session)?;
        let response = self
            .http_client
            .get(format!("{}/status", handle.base_url))
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return Ok(GatewayRuntimeStatus::error(format!(
                    "failed to reach gateway: {error}"
                )));
            }
        };

        if let Err(error) = response.error_for_status_ref() {
            return Ok(GatewayRuntimeStatus::error(format!(
                "failed to reach gateway: {error}"
            )));
        }

        let payload = response.json::<GatewayStatusResponse>().await?;
        Ok(GatewayRuntimeStatus::from_response(payload))
    }

    async fn logs(&self, session: &GatewayRuntimeSession) -> Result<Vec<String>, GatewayError> {
        let handle = Self::docker_session(session)?;
        let response = self
            .http_client
            .get(format!("{}/logs", handle.base_url))
            .send()
            .await;

        match response {
            Ok(response) if response.status().is_success() => {
                let payload = response.json::<GatewayLogsResponse>().await?;
                Ok(payload.lines)
            }
            Ok(response) => {
                tracing::debug!(
                    status = response.status().as_u16(),
                    gateway_id = %handle.container_name,
                    "gateway logs endpoint returned error; falling back to Docker logs"
                );
                self.docker
                    .logs(&handle.container_name, GATEWAY_LOG_TAIL_LINES)
                    .await
            }
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    gateway_id = %handle.container_name,
                    "gateway logs endpoint unreachable; falling back to Docker logs"
                );
                self.docker
                    .logs(&handle.container_name, GATEWAY_LOG_TAIL_LINES)
                    .await
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerGatewayConfig {
    pub gateway_image: String,
    pub network: String,
    pub base_url_template: String,
    pub client_service_url: String,
    pub startup_timeout: Duration,
}

impl DockerGatewayConfig {
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_env_vars(|key| env::var(key).ok())
    }

    #[must_use]
    fn from_env_vars(mut var: impl FnMut(&str) -> Option<String>) -> Self {
        let startup_timeout = var("AGENT_HOST_GATEWAY_STARTUP_TIMEOUT")
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|seconds| seconds.is_sign_positive())
            .map_or_else(
                || Duration::from_secs_f64(DEFAULT_GATEWAY_STARTUP_TIMEOUT_SECONDS),
                Duration::from_secs_f64,
            );

        Self {
            gateway_image: var("AGENT_HOST_GATEWAY_IMAGE")
                .unwrap_or_else(|| DEFAULT_GATEWAY_IMAGE.to_owned()),
            network: var("AGENT_HOST_DOCKER_NETWORK")
                .unwrap_or_else(|| DEFAULT_DOCKER_NETWORK.to_owned()),
            base_url_template: var("AGENT_HOST_GATEWAY_BASE_URL_TEMPLATE")
                .unwrap_or_else(|| DEFAULT_GATEWAY_BASE_URL_TEMPLATE.to_owned()),
            client_service_url: var("AGENT_HOST_GATEWAY_CLIENT_SERVICE_URL")
                .unwrap_or_else(|| DEFAULT_GATEWAY_CLIENT_SERVICE_URL.to_owned()),
            startup_timeout,
        }
    }

    #[must_use]
    pub fn base_url_for(&self, container_name: &str) -> String {
        self.base_url_template
            .replace(CONTAINER_NAME_PLACEHOLDER, container_name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerGatewaySession {
    pub container_name: String,
    pub base_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerGatewayContainerSpec {
    pub name: String,
    pub image: String,
    pub network: String,
    pub entrypoint: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
}

#[async_trait]
pub trait DockerGatewayClient: Clone + Debug + Send + Sync + 'static {
    async fn create_container(&self, spec: &DockerGatewayContainerSpec)
    -> Result<(), GatewayError>;

    async fn remove_container(&self, container_name: &str) -> Result<(), GatewayError>;

    async fn logs(&self, container_name: &str, tail: usize) -> Result<Vec<String>, GatewayError>;
}

#[derive(Clone, Debug, Default)]
pub struct BollardDockerGatewayClient {
    docker: Arc<OnceLock<Result<Docker, String>>>,
}

impl BollardDockerGatewayClient {
    fn docker(&self) -> Result<Docker, GatewayError> {
        let result = self
            .docker
            .get_or_init(|| Docker::connect_with_defaults().map_err(|error| error.to_string()));

        match result {
            Ok(docker) => Ok(docker.clone()),
            Err(message) => Err(GatewayError::Runtime {
                message: format!("failed to connect to Docker: {message}"),
            }),
        }
    }
}

#[async_trait]
impl DockerGatewayClient for BollardDockerGatewayClient {
    async fn create_container(
        &self,
        spec: &DockerGatewayContainerSpec,
    ) -> Result<(), GatewayError> {
        let docker = self.docker()?;
        let options = CreateContainerOptionsBuilder::default()
            .name(&spec.name)
            .build();
        let config = ContainerCreateBody {
            image: Some(spec.image.clone()),
            env: Some(environment_entries(&spec.environment)),
            entrypoint: Some(spec.entrypoint.clone()),
            labels: Some(hash_map_from_btree(&spec.labels)),
            host_config: Some(HostConfig {
                auto_remove: Some(true),
                network_mode: Some(spec.network.clone()),
                ..HostConfig::default()
            }),
            ..ContainerCreateBody::default()
        };

        docker.create_container(Some(options), config).await?;
        docker
            .start_container(
                &spec.name,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await?;
        Ok(())
    }

    async fn remove_container(&self, container_name: &str) -> Result<(), GatewayError> {
        let docker = self.docker()?;
        let options = RemoveContainerOptionsBuilder::default().force(true).build();

        match docker.remove_container(container_name, Some(options)).await {
            Ok(()) => Ok(()),
            Err(error) if is_bollard_not_found(&error) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn logs(&self, container_name: &str, tail: usize) -> Result<Vec<String>, GatewayError> {
        let docker = self.docker()?;
        let tail = tail.to_string();
        let options = LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .tail(&tail)
            .build();
        let mut stream = docker.logs(container_name, Some(options));
        let mut raw = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            append_log_output(&mut raw, &chunk);
        }

        Ok(String::from_utf8_lossy(&raw)
            .lines()
            .map(ToOwned::to_owned)
            .collect())
    }
}

#[derive(Debug)]
pub enum GatewayError {
    NotFound { gateway_id: String },
    AlreadyExists { gateway_id: String },
    InvalidGatewayId { gateway_id: String },
    Runtime { message: String },
    Docker { source: BollardError },
    Http { source: reqwest::Error },
}

impl Display for GatewayError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { gateway_id } => write!(formatter, "gateway {gateway_id} not found"),
            Self::AlreadyExists { gateway_id } => {
                write!(formatter, "gateway {gateway_id} already exists")
            }
            Self::InvalidGatewayId { gateway_id } => write!(
                formatter,
                "invalid gateway_id {gateway_id:?}; expected lower-case hyphenated letters"
            ),
            Self::Runtime { message } => write!(formatter, "gateway runtime error: {message}"),
            Self::Docker { source } => write!(formatter, "gateway Docker error: {source}"),
            Self::Http { source } => write!(formatter, "gateway HTTP error: {source}"),
        }
    }
}

impl Error for GatewayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Docker { source } => Some(source),
            Self::Http { source } => Some(source),
            Self::NotFound { .. }
            | Self::AlreadyExists { .. }
            | Self::InvalidGatewayId { .. }
            | Self::Runtime { .. } => None,
        }
    }
}

impl From<BollardError> for GatewayError {
    fn from(error: BollardError) -> Self {
        Self::Docker { source: error }
    }
}

impl From<reqwest::Error> for GatewayError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http { source: error }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gateways", post(create_gateway).get(list_gateways))
        .route(
            "/gateways/{gateway_id}",
            get(get_gateway).delete(destroy_gateway),
        )
        .route("/gateways/{gateway_id}/logs", get(gateway_logs))
}

async fn create_gateway(
    State(state): State<AppState>,
    Json(payload): Json<GatewayRuntimeCreateRequest>,
) -> Result<Json<GatewaySummary>, GatewayHttpError> {
    state
        .gateways
        .create_gateway(payload)
        .await
        .map(Json)
        .map_err(GatewayHttpError)
}

async fn list_gateways(State(state): State<AppState>) -> Json<Vec<GatewaySummary>> {
    Json(state.gateways.list_gateways().await)
}

async fn get_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<GatewaySummary>, GatewayHttpError> {
    state
        .gateways
        .get_gateway(&gateway_id)
        .await
        .map(Json)
        .map_err(GatewayHttpError)
}

async fn gateway_logs(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<GatewayLogsResponse>, GatewayHttpError> {
    state
        .gateways
        .gateway_logs(&gateway_id)
        .await
        .map(|lines| Json(GatewayLogsResponse { lines }))
        .map_err(GatewayHttpError)
}

async fn destroy_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<StatusCode, GatewayHttpError> {
    state
        .gateways
        .destroy_gateway(&gateway_id)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(GatewayHttpError)
}

#[derive(Debug)]
struct GatewayHttpError(GatewayError);

impl IntoResponse for GatewayHttpError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            GatewayError::NotFound { .. } => StatusCode::NOT_FOUND,
            GatewayError::AlreadyExists { .. } => StatusCode::CONFLICT,
            GatewayError::InvalidGatewayId { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            GatewayError::Runtime { .. } | GatewayError::Docker { .. } => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            GatewayError::Http { .. } => StatusCode::BAD_GATEWAY,
        };

        (status, Json(json!({ "detail": self.0.to_string() }))).into_response()
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct GatewayLogsResponse {
    #[serde(default)]
    lines: Vec<String>,
}

fn validate_gateway_id(gateway_id: &str) -> Result<(), GatewayError> {
    let valid = !gateway_id.is_empty()
        && gateway_id.split('-').all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_lowercase())
        });

    if valid {
        Ok(())
    } else {
        Err(GatewayError::InvalidGatewayId {
            gateway_id: gateway_id.to_owned(),
        })
    }
}

fn environment_entries(environment: &BTreeMap<String, String>) -> Vec<String> {
    environment
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

fn hash_map_from_btree(map: &BTreeMap<String, String>) -> HashMap<String, String> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn append_log_output(buffer: &mut Vec<u8>, output: &LogOutput) {
    buffer.extend_from_slice(output.as_ref());
}

const fn is_bollard_not_found(error: &BollardError) -> bool {
    matches!(
        error,
        BollardError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode, header::CONTENT_TYPE},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    use super::{
        DockerGatewayClient, DockerGatewayConfig, DockerGatewayContainerSpec, DockerGatewayRuntime,
        GATEWAY_ENTRYPOINT, GatewayError, GatewayHost, GatewayRegistry, GatewayRuntime,
        GatewayRuntimeCreateRequest, GatewayRuntimeSession, GatewayRuntimeStatus, GatewayStatus,
    };
    use crate::{AppConfig, AppState, build_router};

    #[derive(Clone, Debug, Default)]
    struct FakeGatewayRuntime {
        state: Arc<Mutex<FakeGatewayRuntimeState>>,
    }

    #[derive(Debug, Default)]
    struct FakeGatewayRuntimeState {
        created: Vec<GatewayRuntimeCreateRequest>,
        destroyed: Vec<String>,
    }

    #[async_trait::async_trait]
    impl GatewayRuntime for FakeGatewayRuntime {
        async fn create_gateway(
            &self,
            request: GatewayRuntimeCreateRequest,
        ) -> Result<GatewayRuntimeSession, GatewayError> {
            let mut state = self.state.lock().await;
            state.created.push(request);
            let handle = format!("container-{}", state.created.len());
            drop(state);
            Ok(GatewayRuntimeSession::opaque(handle))
        }

        async fn destroy_gateway(
            &self,
            session: &GatewayRuntimeSession,
        ) -> Result<(), GatewayError> {
            let GatewayRuntimeSession::Opaque(handle) = session else {
                panic!("fake runtime received unsupported session");
            };
            self.state.lock().await.destroyed.push(handle.clone());
            Ok(())
        }

        async fn status(
            &self,
            _session: &GatewayRuntimeSession,
        ) -> Result<GatewayRuntimeStatus, GatewayError> {
            Ok(GatewayRuntimeStatus::running())
        }

        async fn logs(
            &self,
            _session: &GatewayRuntimeSession,
        ) -> Result<Vec<String>, GatewayError> {
            Ok(vec!["line-1".to_owned(), "line-2".to_owned()])
        }
    }

    #[derive(Clone, Debug, Default)]
    struct FakeDockerGatewayClient {
        state: Arc<Mutex<FakeDockerGatewayClientState>>,
    }

    #[derive(Debug, Default)]
    struct FakeDockerGatewayClientState {
        created: Vec<DockerGatewayContainerSpec>,
        removed: Vec<String>,
        logs: Vec<String>,
    }

    #[async_trait::async_trait]
    impl DockerGatewayClient for FakeDockerGatewayClient {
        async fn create_container(
            &self,
            spec: &DockerGatewayContainerSpec,
        ) -> Result<(), GatewayError> {
            self.state.lock().await.created.push(spec.clone());
            Ok(())
        }

        async fn remove_container(&self, container_name: &str) -> Result<(), GatewayError> {
            self.state
                .lock()
                .await
                .removed
                .push(container_name.to_owned());
            Ok(())
        }

        async fn logs(
            &self,
            _container_name: &str,
            _tail: usize,
        ) -> Result<Vec<String>, GatewayError> {
            Ok(self.state.lock().await.logs.clone())
        }
    }

    #[tokio::test]
    async fn host_create_list_get_logs_and_destroy_gateway() {
        let runtime = FakeGatewayRuntime::default();
        let host = GatewayHost::new(runtime.clone());
        let summary = host
            .create_gateway(gateway_request("echo-one"))
            .await
            .unwrap_or_else(|error| panic!("failed to create gateway: {error}"));

        assert_eq!(summary.gateway_id, "echo-one");
        assert_eq!(summary.status, GatewayStatus::Running);
        assert_eq!(summary.last_error, None);
        assert_eq!(
            runtime.state.lock().await.created[0].env,
            BTreeMap::from([("FOO".to_owned(), "bar".to_owned())])
        );

        let listing = host.list_gateways().await;
        assert_eq!(gateway_ids(&listing), vec!["echo-one"]);

        let fetched = host
            .get_gateway("echo-one")
            .await
            .unwrap_or_else(|error| panic!("failed to fetch gateway: {error}"));
        assert_eq!(fetched.agent_id, "agent-a");

        let logs = host
            .gateway_logs("echo-one")
            .await
            .unwrap_or_else(|error| panic!("failed to fetch logs: {error}"));
        assert_eq!(logs, vec!["line-1", "line-2"]);

        host.destroy_gateway("echo-one")
            .await
            .unwrap_or_else(|error| panic!("failed to destroy gateway: {error}"));
        assert_eq!(
            runtime.state.lock().await.destroyed,
            vec!["container-1".to_owned()]
        );
        assert!(host.list_gateways().await.is_empty());
    }

    #[tokio::test]
    async fn duplicate_gateway_id_rejects_before_runtime_creation() {
        let runtime = FakeGatewayRuntime::default();
        let host = GatewayHost::new(runtime.clone());

        host.create_gateway(gateway_request("dup-gw"))
            .await
            .unwrap_or_else(|error| panic!("failed to create gateway: {error}"));
        let Err(error) = host.create_gateway(gateway_request("dup-gw")).await else {
            panic!("duplicate gateway should fail");
        };

        assert!(matches!(error, GatewayError::AlreadyExists { .. }));
        assert_eq!(runtime.state.lock().await.created.len(), 1);
    }

    #[tokio::test]
    async fn destroy_missing_gateway_returns_not_found() {
        let host = GatewayHost::new(FakeGatewayRuntime::default());
        let Err(error) = host.destroy_gateway("missing").await else {
            panic!("destroying a missing gateway should fail");
        };

        assert!(matches!(error, GatewayError::NotFound { .. }));
    }

    #[tokio::test]
    async fn gateway_routes_match_python_lifecycle_contract() {
        let app = router_with_gateway_runtime(FakeGatewayRuntime::default());
        let payload = json!({
            "gateway_id": "echo-one",
            "gateway_type": "echo",
            "agent_id": "agent-x",
            "env": {"FOO": "bar"},
        });

        let (created_status, created) =
            json_request(&app, Method::POST, "/gateways", payload).await;
        let (listed_status, listed) = empty_request(&app, Method::GET, "/gateways").await;
        let (fetched_status, _fetched) =
            empty_request(&app, Method::GET, "/gateways/echo-one").await;
        let (logs_status, logs) = empty_request(&app, Method::GET, "/gateways/echo-one/logs").await;
        let deleted_status = status_request(&app, Method::DELETE, "/gateways/echo-one").await;
        let after_status = status_request(&app, Method::GET, "/gateways/echo-one").await;

        assert_eq!(created_status, StatusCode::OK);
        assert_eq!(created["gateway_id"], "echo-one");
        assert_eq!(created["status"], "running");
        assert_eq!(listed_status, StatusCode::OK);
        assert_eq!(listed[0]["gateway_id"], "echo-one");
        assert_eq!(fetched_status, StatusCode::OK);
        assert_eq!(logs_status, StatusCode::OK);
        assert_eq!(logs["lines"], json!(["line-1", "line-2"]));
        assert_eq!(deleted_status, StatusCode::NO_CONTENT);
        assert_eq!(after_status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn duplicate_gateway_route_returns_conflict() {
        let app = router_with_gateway_runtime(FakeGatewayRuntime::default());
        let payload = json!({
            "gateway_id": "dup-gw",
            "gateway_type": "echo",
            "agent_id": "agent",
            "env": {},
        });

        let (first_status, _first) =
            json_request(&app, Method::POST, "/gateways", payload.clone()).await;
        let (second_status, _second) = json_request(&app, Method::POST, "/gateways", payload).await;

        assert_eq!(first_status, StatusCode::OK);
        assert_eq!(second_status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn invalid_gateway_id_route_returns_unprocessable_entity() {
        let app = router_with_gateway_runtime(FakeGatewayRuntime::default());
        let payload = json!({
            "gateway_id": "Bad Gateway",
            "gateway_type": "echo",
            "agent_id": "agent",
            "env": {},
        });

        let (status, _body) = json_request(&app, Method::POST, "/gateways", payload).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn docker_gateway_config_uses_python_defaults() {
        let config = DockerGatewayConfig::from_env_vars(|_key| None);

        assert_eq!(config.gateway_image, "agentspace-gateway-gateway:latest");
        assert_eq!(config.network, "agentspace-stack");
        assert_eq!(config.base_url_template, "http://{container_name}:8000");
        assert_eq!(config.client_service_url, "http://client-service:8002");
        assert_eq!(config.startup_timeout, Duration::from_mins(1));
    }

    #[test]
    fn docker_gateway_container_spec_matches_python_runtime() {
        let mut config = DockerGatewayConfig::from_env_vars(|_key| None);
        config.gateway_image = "custom-image".to_owned();
        config.network = "custom-network".to_owned();
        config.client_service_url = "http://client-service.example".to_owned();
        let runtime = DockerGatewayRuntime::new(config, FakeDockerGatewayClient::default());

        let spec =
            runtime.container_spec(&gateway_request("echo-one"), "agentspace-gateway-echo-one");

        assert_eq!(spec.name, "agentspace-gateway-echo-one");
        assert_eq!(spec.image, "custom-image");
        assert_eq!(spec.network, "custom-network");
        assert_eq!(
            spec.entrypoint,
            GATEWAY_ENTRYPOINT
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert_eq!(spec.environment["GATEWAY_ID"], "echo-one");
        assert_eq!(spec.environment["GATEWAY_TYPE"], "echo");
        assert_eq!(spec.environment["GATEWAY_AGENT_ID"], "agent-a");
        assert_eq!(
            spec.environment["GATEWAY_CLIENT_SERVICE_BASE_URL"],
            "http://client-service.example"
        );
        assert_eq!(spec.environment["FOO"], "bar");
        assert_eq!(spec.labels["agentspace.role"], "gateway");
        assert_eq!(spec.labels["agentspace.gateway_id"], "echo-one");
        assert_eq!(spec.labels["agentspace.gateway_type"], "echo");
    }

    fn gateway_request(gateway_id: &str) -> GatewayRuntimeCreateRequest {
        GatewayRuntimeCreateRequest {
            gateway_id: gateway_id.to_owned(),
            gateway_type: "echo".to_owned(),
            agent_id: "agent-a".to_owned(),
            env: BTreeMap::from([("FOO".to_owned(), "bar".to_owned())]),
        }
    }

    fn gateway_ids(summaries: &[super::GatewaySummary]) -> Vec<&str> {
        summaries
            .iter()
            .map(|summary| summary.gateway_id.as_str())
            .collect()
    }

    fn router_with_gateway_runtime(runtime: impl GatewayRuntime) -> Router {
        let mut state = AppState::new(AppConfig::new("127.0.0.1", 0, BTreeMap::new()));
        state.gateways = GatewayRegistry::new(runtime);
        build_router(state)
    }

    async fn json_request(
        app: &Router,
        method: Method,
        uri: &str,
        payload: Value,
    ) -> (StatusCode, Value) {
        let body = serde_json::to_vec(&payload)
            .unwrap_or_else(|error| panic!("failed to serialize request body: {error}"));
        request(app, method, uri, Body::from(body), true).await
    }

    async fn empty_request(app: &Router, method: Method, uri: &str) -> (StatusCode, Value) {
        request(app, method, uri, Body::empty(), false).await
    }

    async fn status_request(app: &Router, method: Method, uri: &str) -> StatusCode {
        let (status, _body) = request(app, method, uri, Body::empty(), false).await;
        status
    }

    async fn request(
        app: &Router,
        method: Method,
        uri: &str,
        body: Body,
        json_body: bool,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if json_body {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        let request = builder
            .body(body)
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = app
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
            .to_bytes();
        let payload = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("failed to parse response body: {error}"))
        };

        (status, payload)
    }
}
