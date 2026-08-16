use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{DefaultBodyLimit, OriginalUri, Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{sync::mpsc, time::sleep};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::{
    ActiveTurnRecord, ActiveTurnStreamState, AppState, ENV_PREFIX, StreamItem,
    agent_host::{AgentHostError, JsonObject, KernelEvent},
    errors::{StoreError, ValidationError},
    memory::{MEMORY_JSON_CONTENT_TYPE, MEMORY_RUN_CONTENT_TYPE, MemoryProxyError},
    models::{
        AdditionalPathIdentity, AgentCliRecord, AgentProfileLaunchSnapshot, AgentRecord,
        CliHarnessName, CliLaunchOptionsSnapshot, CliLaunchSnapshot, CliProviderLaunchSnapshot,
        ClientType, ConnectionApiFlavor, ConnectionRecord, DEFAULT_AGENT_SYSTEM_PROMPT,
        GatewayRecord, GatewayType, HarnessName, InteractionMode, LaunchValueSource, MessageRecord,
        MessageRole, RuntimeStatus, SessionRecord, ToolCallRecord, WorkspaceMountRecord,
        WorkspaceRecord, WorkspaceStatus, utc_now, validate_agent_id, validate_connection_id,
        validate_gateway_id, validate_skill_id, validate_workspace_id,
    },
};

use crate::config::{
    self,
    canonical::to_canonical_yaml,
    document::{Agent as ConfigAgent, SecretDeclaration},
    error::{ConfigError, ValidationIssue},
    resolver::ResolveError,
    secrets::SecretStoreError,
    snapshot::SourceKind,
    value::{ConfigValue, SecretName},
};

const DEFAULT_AGENTSPACE_CLIENT_SERVICE_URL: &str = "http://client-service:8002";
const AGENTSPACE_CLIENT_SERVICE_URL_ENV: &str = "CLIENT_SERVICE_AGENTSPACE_BASE_URL";
const GATEWAY_AUTOSTART_ATTEMPTS: usize = 5;
const GATEWAY_AUTOSTART_RETRY_DELAY: Duration = Duration::from_secs(2);
const MEMORY_MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MEMORY_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
/// Request body limit for the config endpoints that accept uploaded bundles.
///
/// A config bundle is a ZIP whose decompressed contents may total up to 32 MiB
/// (`bundle::MAX_TOTAL_BYTES`). The compressed upload is smaller, but we set the
/// limit comfortably above the declared bundle size so large-but-valid bundles
/// are accepted while other routes keep the framework's smaller default limit.
const CONFIG_BODY_LIMIT_BYTES: usize = 40 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/info", get(info))
        .route("/harnesses", get(list_harnesses))
        .route("/kernel-configs", get(list_kernel_configs))
        .route(
            "/kernel-configs/{harness}",
            get(get_kernel_config).put(update_kernel_config),
        )
        .route(
            "/connections",
            get(list_connections).post(create_connection),
        )
        .route(
            "/connections/{connection_id}",
            get(get_connection)
                .patch(update_connection)
                .delete(delete_connection),
        )
        .route(
            "/connections/{connection_id}/models",
            get(list_connection_models),
        )
        .route("/agents", get(list_agents).post(create_agent))
        .route(
            "/agents/{agent_id}",
            get(get_agent).patch(update_agent).delete(delete_agent),
        )
        .merge(memory_router())
        .route("/workspaces", get(list_workspaces).post(create_workspace))
        .route(
            "/workspaces/{workspace_id}",
            get(get_workspace)
                .patch(update_workspace)
                .delete(delete_workspace),
        )
        .route("/workspaces/{workspace_id}/clone", post(clone_workspace))
        .route(
            "/workspaces/{workspace_id}/vscode",
            post(open_workspace_vscode),
        )
        .route("/sessions", get(list_sessions).post(create_session))
        .route(
            "/sessions/{session_id}",
            get(get_session).delete(delete_session),
        )
        .route(
            "/sessions/{session_id}/messages",
            get(list_messages).post(send_message),
        )
        .route(
            "/sessions/{session_id}/messages/stream",
            post(stream_message),
        )
        .route(
            "/sessions/{session_id}/workspace/save",
            post(save_session_workspace),
        )
        .route(
            "/sessions/{session_id}/turns/{turn_id}/stream",
            get(stream_turn),
        )
        .route("/sessions/{session_id}/reset", post(reset_session))
        .route("/kernels", get(list_kernels))
        .route("/kernels/{kernel_session_id}", delete(kill_kernel))
        .route("/kernels/{kernel_session_id}/logs", get(kernel_logs))
        .route(
            "/kernels/{kernel_session_id}/container-logs",
            get(kernel_container_logs),
        )
        .route("/skills", get(list_skills).post(create_skill))
        .route("/skills/{skill_id}/versions", get(list_skill_versions))
        .route("/skills/{skill_id}/download", get(download_skill))
        .route(
            "/skills/{skill_id}/versions/{version}/rollback",
            post(rollback_skill_version),
        )
        .route(
            "/skills/{skill_id}",
            get(get_skill).put(update_skill).delete(delete_skill),
        )
        .route("/gateway-types", get(list_gateway_types))
        .route(
            "/gateway-types/{gateway_type}/schema",
            get(get_gateway_type_schema),
        )
        .route("/gateways", get(list_gateways).post(create_gateway))
        .route(
            "/gateways/{gateway_id}",
            get(get_gateway)
                .patch(update_gateway)
                .delete(delete_gateway),
        )
        .route("/gateways/{gateway_id}/start", post(start_gateway))
        .route("/gateways/{gateway_id}/stop", post(stop_gateway))
        .route("/gateways/{gateway_id}/logs", get(gateway_logs))
        .merge(config_router())
}

fn config_router() -> Router<AppState> {
    // Bundle-accepting endpoints may receive large ZIP uploads, so they get an
    // explicit body limit above the declared bundle size. The remaining config
    // endpoints (export + secrets) keep the framework's smaller default limit.
    let bundle_routes = Router::new()
        .route("/config/validate", post(validate_config))
        .route("/config/plan", post(plan_config))
        .route("/config/apply", post(apply_config))
        .layer(DefaultBodyLimit::max(CONFIG_BODY_LIMIT_BYTES));

    Router::new()
        .route("/config/export", get(export_config))
        .route("/config/export/{kind}/{name}", get(export_config_resource))
        .route("/secrets", get(list_secrets).post(create_secret))
        .route("/secrets/{name}", delete(delete_secret))
        .route(
            "/secrets/{name}/value",
            axum::routing::put(set_secret_value).delete(clear_secret_value),
        )
        .merge(bundle_routes)
}

fn memory_router() -> Router<AppState> {
    Router::new()
        .route("/memory/healthz", get(proxy_memory))
        .route("/memory/v1/pages", get(proxy_memory))
        .route(
            "/memory/v1/pages/content",
            get(proxy_memory).put(proxy_memory).delete(proxy_memory),
        )
        .route("/memory/v1/pages/move", post(proxy_memory))
        .route("/memory/v1/tags", get(proxy_memory))
        .route("/memory/v1/links", get(proxy_memory))
        .route("/memory/v1/check", get(proxy_memory))
        .route("/memory/v1/run", post(proxy_memory))
}

async fn proxy_memory(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let upstream_path = uri
        .path()
        .strip_prefix("/memory")
        .ok_or_else(|| ApiError::internal("invalid memory proxy route".to_owned()))?;
    let request_body = to_bytes(body, MEMORY_MAX_REQUEST_BYTES)
        .await
        .map_err(|error| ApiError::payload_too_large(error.to_string()))?
        .to_vec();
    let response = state
        .memory
        .request(
            method,
            upstream_path,
            uri.query(),
            headers.get(header::CONTENT_TYPE),
            request_body,
            upstream_path == "/v1/run",
        )
        .await?;
    let status = response.status();

    if upstream_path == "/v1/run" && status.is_success() {
        require_memory_content_type(&response, MEMORY_RUN_CONTENT_TYPE)?;
        return Ok(stream_memory_response(status, response));
    }

    if status == StatusCode::NO_CONTENT {
        return Ok(status.into_response());
    }

    require_memory_content_type(&response, MEMORY_JSON_CONTENT_TYPE)?;
    let bytes = collect_memory_response(response).await?;
    serde_json::from_slice::<Value>(&bytes).map_err(|error| {
        MemoryProxyError::MalformedResponse {
            detail: error.to_string(),
        }
    })?;
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, MEMORY_JSON_CONTENT_TYPE)
        .body(Body::from(bytes))
        .map_err(|error| ApiError::internal(error.to_string()))
}

fn require_memory_content_type(
    response: &reqwest::Response,
    expected: &'static str,
) -> Result<(), MemoryProxyError> {
    let actual = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if actual == Some(expected) {
        return Ok(());
    }
    Err(MemoryProxyError::MalformedResponse {
        detail: format!("expected content-type {expected:?}, got {actual:?}"),
    })
}

async fn collect_memory_response(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, MemoryProxyError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| MemoryProxyError::Http { source })?
    {
        if body.len().saturating_add(chunk.len()) > MEMORY_MAX_RESPONSE_BYTES {
            return Err(MemoryProxyError::ResponseTooLarge {
                limit: MEMORY_MAX_RESPONSE_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn stream_memory_response(status: StatusCode, mut response: reqwest::Response) -> Response {
    let (sender, receiver) = mpsc::channel::<StreamItem>(8);
    tokio::spawn(async move {
        loop {
            let item = match response.chunk().await {
                Ok(Some(chunk)) => Ok(chunk.to_vec()),
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(%error, "memory run upstream stream failed");
                    break;
                }
            };
            if sender.send(item).await.is_err() {
                break;
            }
        }
    });
    (
        status,
        [(header::CONTENT_TYPE, MEMORY_RUN_CONTENT_TYPE)],
        Body::from_stream(ReceiverStream::new(receiver)),
    )
        .into_response()
}

async fn healthz() -> Json<HealthResponse> {
    tracing::debug!(
        route = "/healthz",
        action = "healthz",
        "api handler completed"
    );
    Json(HealthResponse { status: "ok" })
}

async fn info(State(state): State<AppState>) -> Json<Value> {
    let agent_host = match state.agent_host.info().await {
        Ok(info) => {
            tracing::info!(
                route = "/info",
                action = "info",
                agent_host_available = true,
                "api handler completed"
            );
            Value::Object(info)
        }
        Err(error) => {
            tracing::warn!(
                route = "/info",
                action = "info",
                agent_host_available = false,
                error_kind = "agent_host_info_failed",
                "agent_host info unavailable"
            );
            json!({ "service": "agent_host", "error": error.to_string() })
        }
    };

    Json(json!({
        "client_service": {
            "service": "client_service",
            "title": "Client Service",
            "version": env!("CARGO_PKG_VERSION"),
            "env_prefix": ENV_PREFIX,
            "env": state.config.redacted_env(),
            "instance_id": state.instance_id,
            "started_at": state.started_at,
        },
        "agent_host": agent_host,
    }))
}

async fn list_harnesses() -> Json<Vec<&'static str>> {
    let harnesses = HarnessName::all()
        .iter()
        .map(|harness| harness.as_str())
        .collect::<Vec<_>>();
    tracing::info!(
        route = "/harnesses",
        action = "list_harnesses",
        harness_count = harnesses.len(),
        "api handler completed"
    );
    Json(harnesses)
}

async fn list_kernel_configs(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let configs = state
        .kernel_configs
        .list()?
        .into_iter()
        .map(|record| record.summary())
        .collect::<Vec<_>>();
    tracing::info!(
        route = "/kernel-configs",
        action = "list_kernel_configs",
        config_count = configs.len(),
        "api handler completed"
    );
    Ok(Json(configs))
}

async fn get_kernel_config(
    State(state): State<AppState>,
    Path(raw_harness): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let harness = parse_harness(&raw_harness)?;
    let value = state.kernel_configs.get(harness)?.map_or_else(
        || json!({ "harness": harness.as_str(), "env_vars": "", "updated_at": null }),
        |record| record.summary(),
    );
    tracing::info!(
        route = "/kernel-configs/:harness",
        action = "get_kernel_config",
        harness = harness.as_str(),
        configured = !value["updated_at"].is_null(),
        "api handler completed"
    );
    Ok(Json(value))
}

async fn update_kernel_config(
    State(state): State<AppState>,
    Path(raw_harness): Path<String>,
    Json(payload): Json<UpdateKernelConfigRequest>,
) -> Result<Json<Value>, ApiError> {
    let harness = parse_harness(&raw_harness)?;
    let record = state.kernel_configs.upsert(harness, payload.env_vars)?;
    tracing::info!(
        route = "/kernel-configs/:harness",
        action = "update_kernel_config",
        harness = harness.as_str(),
        "api handler completed"
    );
    Ok(Json(record.summary()))
}

async fn list_connections(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let connections = state
        .connections
        .list()?
        .into_iter()
        .map(|connection| connection.summary(false))
        .collect::<Vec<_>>();
    tracing::info!(
        route = "/connections",
        action = "list_connections",
        connection_count = connections.len(),
        "api handler completed"
    );
    Ok(Json(connections))
}

async fn get_connection(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let connection = require_connection(&state, &connection_id)?;
    tracing::info!(
        route = "/connections/:connection_id",
        action = "get_connection",
        connection_id = %connection_id,
        api_flavor = connection.api_flavor.as_str(),
        has_api_key = connection.has_api_key(),
        "api handler completed"
    );
    Ok(Json(connection.summary(false)))
}

async fn list_connection_models(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let connection = require_connection(&state, &connection_id)?;
    let mut missing = Vec::new();
    let resolved =
        config::resolver::resolve_connection(&state.config_state, &connection_id, &mut missing)
            .map_err(resolve_error_to_api)?;
    if !missing.is_empty() {
        return Err(resolve_error_to_api(ResolveError::Missing(missing)));
    }
    let base_url = resolved.url.ok_or_else(|| {
        ApiError::unprocessable(format!(
            "connection {connection_id} has no resolvable URL configured"
        ))
    })?;
    let api_key = resolved.api_key;
    tracing::info!(
        route = "/connections/:connection_id/models",
        action = "list_connection_models",
        connection_id = %connection_id,
        api_flavor = connection.api_flavor.as_str(),
        "fetching connection models"
    );
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut request = state
        .http_client
        .get(url)
        .timeout(state.config.connection_models_timeout());
    if let Some(api_key) = api_key.filter(|value| !value.is_empty()) {
        request = request.bearer_auth(&api_key);
    }
    let response = request.send().await.map_err(|error| {
        ApiError::bad_gateway(format!(
            "failed to fetch models for connection {connection_id}: {error}"
        ))
    })?;
    let response = if response.status().is_success() {
        tracing::info!(
            route = "/connections/:connection_id/models",
            action = "list_connection_models",
            connection_id = %connection_id,
            upstream_status = response.status().as_u16(),
            "connection models fetched"
        );
        response
    } else {
        tracing::warn!(
            route = "/connections/:connection_id/models",
            action = "list_connection_models",
            connection_id = %connection_id,
            upstream_status = response.status().as_u16(),
            error_kind = "upstream_http_status",
            "connection models fetch failed"
        );
        return Err(ApiError::bad_gateway(format!(
            "failed to fetch models for connection {connection_id}: HTTP {}",
            response.status()
        )));
    };
    let value = response.json::<Value>().await.map_err(|error| {
        ApiError::bad_gateway(format!(
            "models response for connection {connection_id} was not valid JSON: {error}"
        ))
    })?;
    if !value.is_object() {
        tracing::warn!(
            route = "/connections/:connection_id/models",
            action = "list_connection_models",
            connection_id = %connection_id,
            error_kind = "invalid_upstream_shape",
            "connection models response was not an object"
        );
        return Err(ApiError::bad_gateway(format!(
            "models response for connection {connection_id} was not a JSON object"
        )));
    }
    tracing::info!(
        route = "/connections/:connection_id/models",
        action = "list_connection_models",
        connection_id = %connection_id,
        "api handler completed"
    );
    Ok(Json(value))
}

async fn create_connection(
    State(state): State<AppState>,
    Json(payload): Json<CreateConnectionRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_connection_id(&payload.connection_id)?;
    let mut connection = ConnectionRecord::new(payload.connection_id, payload.name, payload.url);
    connection.api_flavor = payload.api_flavor;
    apply_connection_api_key(
        &mut connection,
        payload.api_key,
        payload.api_key_secret.as_deref(),
    )?;
    let value = connection.summary(true);
    write_connection(&state, &connection, |record| {
        state.connections.insert(record)
    })?;
    tracing::info!(
        route = "/connections",
        action = "create_connection",
        connection_id = %value["connection_id"].as_str().unwrap_or_default(),
        api_flavor = %value["api_flavor"].as_str().unwrap_or_default(),
        has_api_key = value["has_api_key"].as_bool().unwrap_or(false),
        api_key_secret = connection.api_key_secret.as_ref().map_or("", SecretName::as_str),
        "api handler completed"
    );
    Ok(Json(value))
}

/// Persist a connection, serializing the write against secret declaration and
/// removal operations.
///
/// The referenced declaration must still exist at the moment the document is
/// mutated. Checking it separately would leave a window in which a concurrent
/// removal turns the write into a document validation failure (a 500) rather
/// than a clean rejection.
fn write_connection<F>(
    state: &AppState,
    connection: &ConnectionRecord,
    write: F,
) -> Result<(), ApiError>
where
    F: FnOnce(&ConnectionRecord) -> Result<(), StoreError>,
{
    let required = connection
        .api_key_secret
        .as_ref()
        .map(SecretName::as_str)
        .map(ToOwned::to_owned);
    let written = state
        .config_state
        .with_declared_secret(required.as_deref(), || write(connection))?;
    if written.is_none() {
        return Err(ApiError::unprocessable(format!(
            "secret {} must be declared before it can be referenced",
            required.unwrap_or_default()
        )));
    }
    Ok(())
}

/// Apply the requested API key selection to a connection record.
///
/// `api_key_secret` names a declared secret and is the only form clients such as
/// the web UI offer; literal keys remain authorable through YAML. Supplying both
/// fields is rejected regardless of their values, an empty value for either
/// clears the key, and omitting both leaves the record untouched.
///
/// The name is only validated for grammar here; that it is actually declared is
/// enforced atomically with the write by [`write_connection`].
fn apply_connection_api_key(
    connection: &mut ConnectionRecord,
    literal: Option<String>,
    secret: Option<&str>,
) -> Result<(), ApiError> {
    if literal.is_some() && secret.is_some() {
        return Err(ApiError::unprocessable(
            "api_key and api_key_secret are mutually exclusive; provide only one".to_owned(),
        ));
    }
    if let Some(secret) = secret {
        let secret_name = secret.trim();
        if secret_name.is_empty() {
            connection.api_key = String::new();
            connection.api_key_secret = None;
            return Ok(());
        }
        let name = SecretName::new(secret_name)
            .map_err(|error| ApiError::unprocessable(error.to_string()))?;
        connection.api_key = String::new();
        connection.api_key_secret = Some(name);
        return Ok(());
    }
    if let Some(literal) = literal {
        connection.api_key = literal;
        connection.api_key_secret = None;
    }
    Ok(())
}

async fn update_connection(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
    Json(payload): Json<UpdateConnectionRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut connection = require_connection(&state, &connection_id)?;
    if let Some(name) = payload.name {
        connection.name = name;
    }
    if let Some(url) = payload.url {
        connection.url = url;
    }
    if let Some(api_flavor) = payload.api_flavor {
        connection.api_flavor = api_flavor;
    }
    apply_connection_api_key(
        &mut connection,
        payload.api_key,
        payload.api_key_secret.as_deref(),
    )?;
    connection.updated_at = utc_now();
    let value = connection.summary(true);
    write_connection(&state, &connection, |record| {
        state.connections.update(record)
    })?;
    tracing::info!(
        route = "/connections/:connection_id",
        action = "update_connection",
        connection_id = %connection_id,
        api_flavor = %value["api_flavor"].as_str().unwrap_or_default(),
        has_api_key = value["has_api_key"].as_bool().unwrap_or(false),
        api_key_secret = connection.api_key_secret.as_ref().map_or("", SecretName::as_str),
        "api handler completed"
    );
    Ok(Json(value))
}

async fn delete_connection(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.connections.delete(&connection_id)? {
        tracing::info!(
            route = "/connections/:connection_id",
            action = "delete_connection",
            connection_id = %connection_id,
            deleted = true,
            "api handler completed"
        );
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!(
            "connection {connection_id:?} not found"
        )))
    }
}

async fn create_agent(
    State(state): State<AppState>,
    Json(payload): Json<CreateAgentRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_agent_id(&payload.agent_id)?;
    validate_agent_skill_refs(&state, &payload.skills).await?;
    if let Some(connection_id) = payload.connection_id.as_deref() {
        require_connection(&state, connection_id)?;
    }
    if let Some(cli) = &payload.cli
        && let Some(connection_id) = cli.connection_id.as_deref()
    {
        require_connection(&state, connection_id)?;
    }
    let mut agent = AgentRecord::new(
        payload.agent_id,
        payload.name,
        payload.harness,
        payload.system_prompt,
    );
    agent.skills = payload.skills;
    agent.env_vars = payload.env_vars;
    agent.connection_id = payload.connection_id;
    agent.cli = payload.cli.map(AgentCliRequest::into_record);
    validate_workspace_mounts(&state, &payload.workspace_mounts)?;
    agent.workspace_mounts = payload.workspace_mounts;
    let value = agent.summary();
    state.agents.insert(&agent)?;
    tracing::info!(
        route = "/agents",
        action = "create_agent",
        agent_id = %value["agent_id"].as_str().unwrap_or_default(),
        harness = %value["harness"].as_str().unwrap_or_default(),
        skill_count = value["skills"].as_array().map_or(0, Vec::len),
        has_connection = value["connection_id"].is_string(),
        "api handler completed"
    );
    Ok(Json(value))
}

async fn list_agents(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let agents = state
        .agents
        .list()?
        .into_iter()
        .map(|agent| agent.summary())
        .collect::<Vec<_>>();
    tracing::info!(
        route = "/agents",
        action = "list_agents",
        agent_count = agents.len(),
        "api handler completed"
    );
    Ok(Json(agents))
}

async fn get_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let agent = require_agent(&state, &agent_id)?;
    tracing::info!(
        route = "/agents/:agent_id",
        action = "get_agent",
        agent_id = %agent_id,
        harness = agent.harness.as_str(),
        skill_count = agent.skills.len(),
        has_connection = agent.connection_id.is_some(),
        "api handler completed"
    );
    Ok(Json(agent.summary()))
}

async fn update_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(payload): Json<UpdateAgentRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut agent = require_agent(&state, &agent_id)?;
    if let Some(name) = payload.name {
        agent.name = name;
    }
    if let Some(harness) = payload.harness {
        agent.harness = harness;
    }
    if let Some(system_prompt) = payload.system_prompt {
        agent.system_prompt = system_prompt;
    }
    if let Some(skills) = payload.skills {
        validate_agent_skill_refs(&state, &skills).await?;
        agent.skills = skills;
    }
    if let Some(env_vars) = payload.env_vars {
        agent.env_vars = env_vars;
    }
    if let Some(workspace_mounts) = payload.workspace_mounts {
        validate_workspace_mounts(&state, &workspace_mounts)?;
        agent.workspace_mounts = workspace_mounts;
    }
    match payload.connection_id {
        NullableStringField::Missing => {}
        NullableStringField::Null => {
            agent.connection_id = None;
        }
        NullableStringField::Value(connection_id) => {
            require_connection(&state, &connection_id)?;
            agent.connection_id = Some(connection_id);
        }
    }
    match payload.cli {
        NullableCliField::Missing => {}
        NullableCliField::Null => {
            agent.cli = None;
        }
        NullableCliField::Value(cli) => {
            if let Some(connection_id) = cli.connection_id.as_deref() {
                require_connection(&state, connection_id)?;
            }
            agent.cli = Some(cli.into_record());
        }
    }
    agent.updated_at = utc_now();
    let value = agent.summary();
    state.agents.update(&agent)?;
    tracing::info!(
        route = "/agents/:agent_id",
        action = "update_agent",
        agent_id = %agent_id,
        harness = %value["harness"].as_str().unwrap_or_default(),
        skill_count = value["skills"].as_array().map_or(0, Vec::len),
        has_connection = value["connection_id"].is_string(),
        "api handler completed"
    );
    Ok(Json(value))
}

async fn delete_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if !state.agents.delete(&agent_id)? {
        return Err(ApiError::not_found(format!("agent {agent_id:?} not found")));
    }
    let sessions = state.sessions.list()?;
    for session in sessions
        .into_iter()
        .filter(|session| session.agent_id == agent_id)
    {
        tracing::info!(
            route = "/agents/:agent_id",
            action = "delete_agent",
            agent_id = %agent_id,
            session_id = %session.session_id,
            kernel_session_id = %session.agent_host_session_id,
            "destroying session for deleted agent"
        );
        let _removed = state.sessions.delete(&session.session_id)?;
        if session.interaction_mode == InteractionMode::Chat {
            state
                .agent_host
                .destroy_session(&session.agent_host_session_id)
                .await?;
        }
    }
    tracing::info!(
        route = "/agents/:agent_id",
        action = "delete_agent",
        agent_id = %agent_id,
        "api handler completed"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn list_workspaces(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let workspaces: Vec<Value> = state
        .workspaces
        .list()?
        .into_iter()
        .map(|workspace| workspace.summary())
        .collect();
    tracing::info!(
        route = "/workspaces",
        action = "list_workspaces",
        workspace_count = workspaces.len(),
        "api handler completed"
    );
    Ok(Json(workspaces))
}

async fn create_workspace(
    State(state): State<AppState>,
    Json(payload): Json<CreateWorkspaceRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_workspace_id(&payload.workspace_id)?;
    let workspace = WorkspaceRecord::new(payload.workspace_id, payload.name);
    let value = workspace.summary();
    state.workspaces.insert(workspace)?;
    tracing::info!(
        route = "/workspaces",
        action = "create_workspace",
        workspace_id = %value["workspace_id"].as_str().unwrap_or_default(),
        "api handler completed"
    );
    Ok(Json(value))
}

async fn get_workspace(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = require_workspace(&state, &workspace_id)?;
    tracing::info!(
        route = "/workspaces/:workspace_id",
        action = "get_workspace",
        workspace_id = %workspace_id,
        "api handler completed"
    );
    Ok(Json(workspace.summary()))
}

async fn update_workspace(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    Json(payload): Json<UpdateWorkspaceRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut workspace = require_workspace(&state, &workspace_id)?;
    if let Some(name) = payload.name {
        workspace.name = name;
    }
    workspace.updated_at = utc_now();
    let value = workspace.summary();
    state.workspaces.update(workspace)?;
    tracing::info!(
        route = "/workspaces/:workspace_id",
        action = "update_workspace",
        workspace_id = %workspace_id,
        "api handler completed"
    );
    Ok(Json(value))
}

async fn delete_workspace(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if workspace_in_use(&state, &workspace_id)? {
        return Err(ApiError::conflict(format!(
            "workspace {workspace_id:?} is mounted by one or more agents"
        )));
    }
    if state.workspaces.delete(&workspace_id)? {
        tracing::info!(
            route = "/workspaces/:workspace_id",
            action = "delete_workspace",
            workspace_id = %workspace_id,
            "api handler completed"
        );
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!(
            "workspace {workspace_id:?} not found"
        )))
    }
}

async fn clone_workspace(
    State(state): State<AppState>,
    Path(source_workspace_id): Path<String>,
    Json(payload): Json<CloneWorkspaceRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_workspace_id(&payload.workspace_id)?;
    let source_workspace = require_ready_workspace(&state, &source_workspace_id)?;
    let mut target_workspace = WorkspaceRecord::new_with_status(
        payload.workspace_id,
        payload.name,
        WorkspaceStatus::Creating,
    );
    let target_workspace_id = target_workspace.workspace_id.clone();
    let target_volume_name = target_workspace.volume_name();
    let source_volume_name = source_workspace.volume_name();
    state.workspaces.insert(target_workspace.clone())?;
    let clone_result = state
        .agent_host
        .clone_workspace(
            &source_volume_name,
            &target_workspace_id,
            &target_volume_name,
        )
        .await;
    if let Err(error) = clone_result {
        target_workspace.status = WorkspaceStatus::Failed;
        target_workspace.updated_at = utc_now();
        state.workspaces.update(target_workspace)?;
        return Err(error.into());
    }
    target_workspace.status = WorkspaceStatus::Ready;
    target_workspace.updated_at = utc_now();
    let value = target_workspace.summary();
    state.workspaces.update(target_workspace)?;
    tracing::info!(
        route = "/workspaces/:workspace_id/clone",
        action = "clone_workspace",
        source_workspace_id = %source_workspace_id,
        target_workspace_id = %target_workspace_id,
        "api handler completed"
    );
    Ok(Json(value))
}

async fn open_workspace_vscode(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let workspace = require_ready_workspace(&state, &workspace_id)?;
    let volume_name = workspace.volume_name();
    let upstream = state
        .agent_host
        .open_workspace_vscode(&workspace.workspace_id, &volume_name)
        .await?;
    tracing::info!(
        route = "/workspaces/:workspace_id/vscode",
        action = "open_workspace_vscode",
        workspace_id = %workspace_id,
        "api handler completed"
    );
    Ok(Json(Value::Object(upstream)))
}

async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<Value>, ApiError> {
    let agent = require_agent(&state, &payload.agent_id)?;
    let session_mounts =
        session_workspace_mounts(&agent.workspace_mounts, &payload.workspace_mounts);
    validate_workspace_mounts(&state, &session_mounts)?;
    if payload.interaction_mode == InteractionMode::Cli {
        return create_cli_session(&state, payload, &agent, &session_mounts);
    }
    let env = session_env(&state, &agent)?;
    tracing::info!(
        route = "/sessions",
        action = "create_session",
        agent_id = %agent.agent_id,
        harness = agent.harness.as_str(),
        skill_count = agent.skills.len(),
        env_var_count = env.len(),
        workspace_mount_count = session_mounts.len(),
        has_connection = agent.connection_id.is_some(),
        "creating upstream session"
    );
    let workspace_mounts = session_mounts.clone();
    let upstream = state
        .agent_host
        .create_session(
            agent.harness.as_str(),
            Some(&agent.skills),
            Some(&env),
            Some(&workspace_mounts),
        )
        .await?;
    let upstream_session_id = string_field(&upstream, "session_id")?;
    let status = string_field(&upstream, "status")?;
    let session = SessionRecord::new(
        Uuid::now_v7().simple().to_string(),
        payload.agent_id,
        upstream_session_id.clone(),
        status.clone(),
        payload.channel_name,
        payload.client_type,
    );
    let value = session_summary(&state, &session)?;
    state.sessions.insert(session)?;
    tracing::info!(
        route = "/sessions",
        action = "create_session",
        session_id = %value["session_id"].as_str().unwrap_or_default(),
        agent_id = %value["agent_id"].as_str().unwrap_or_default(),
        kernel_session_id = %upstream_session_id,
        upstream_status = %status,
        "api handler completed"
    );
    Ok(Json(value))
}

fn create_cli_session(
    state: &AppState,
    payload: CreateSessionRequest,
    agent: &AgentRecord,
    session_mounts: &[WorkspaceMountRecord],
) -> Result<Json<Value>, ApiError> {
    let cli = agent.cli.as_ref().ok_or_else(|| {
        ApiError::unprocessable(format!(
            "agent {:?} is not configured for CLI sessions",
            agent.agent_id
        ))
    })?;
    if let Some(connection_id) = cli.connection_id.as_deref() {
        require_connection(state, connection_id)?;
    }

    let session_id = Uuid::now_v7().simple().to_string();
    let launch_snapshot = cli_launch_snapshot(state, agent, cli, &session_id, session_mounts)?;
    let mut session = SessionRecord::new(
        session_id.clone(),
        payload.agent_id,
        "",
        "starting",
        payload.channel_name,
        payload.client_type,
    );
    session.interaction_mode = InteractionMode::Cli;
    session.cli_harness = Some(cli.harness);
    session.cli_connection_id.clone_from(&cli.connection_id);
    session.harness_session_id = Some(Uuid::new_v4().to_string());
    session.runtime_generation = Some(0);
    session.runtime_status = Some(RuntimeStatus::Starting);
    session.workspace_volume_identity = Some(session_id);
    session.launch_snapshot = Some(launch_snapshot);

    state.sessions.insert(session.clone())?;
    let value = session_summary(state, &session)?;
    tracing::info!(
        route = "/sessions",
        action = "create_session",
        interaction_mode = InteractionMode::Cli.as_str(),
        session_id = %session.session_id,
        agent_id = %session.agent_id,
        cli_harness = cli.harness.as_str(),
        has_cli_connection = cli.connection_id.is_some(),
        runtime_status = RuntimeStatus::Starting.as_str(),
        "durable CLI session created without launching a runtime"
    );
    Ok(Json(value))
}

fn cli_launch_snapshot(
    state: &AppState,
    agent: &AgentRecord,
    cli: &AgentCliRecord,
    session_id: &str,
    session_mounts: &[WorkspaceMountRecord],
) -> Result<CliLaunchSnapshot, ApiError> {
    let document = state.config_state.active();
    let config_agent = document
        .spec
        .agents
        .iter()
        .find(|item| item.id == agent.agent_id)
        .ok_or_else(|| {
            ApiError::internal(format!(
                "agent {:?} has no authoritative config document entry",
                agent.agent_id
            ))
        })?;
    let mut env_sources = BTreeMap::new();
    if let Some(kernel) = document
        .spec
        .kernel_configs
        .iter()
        .find(|item| item.harness == HarnessName::CopilotCli)
    {
        env_sources.extend(config_env_sources(
            kernel.env.as_ref(),
            kernel.env_text.as_deref(),
            "kernelConfigs/copilot-cli",
        ));
    }
    env_sources.extend(config_env_sources(
        config_agent.env.as_ref(),
        config_agent.env_text.as_deref(),
        &format!("agents/{}", agent.agent_id),
    ));

    let provider = cli
        .connection_id
        .as_deref()
        .map(|connection_id| {
            let connection = document.connection(connection_id).ok_or_else(|| {
                ApiError::unprocessable(format!(
                    "agent {:?} CLI references unknown connection {connection_id:?}",
                    agent.agent_id
                ))
            })?;
            Ok::<CliProviderLaunchSnapshot, ApiError>(CliProviderLaunchSnapshot {
                provider_type: "openai".to_owned(),
                wire_api: match connection.api_flavor {
                    ConnectionApiFlavor::ChatCompletions => "completions",
                    ConnectionApiFlavor::Responses => "responses",
                }
                .to_owned(),
                connection_id: connection_id.to_owned(),
                base_url: launch_value_source(
                    &connection.url,
                    &format!("connections/{connection_id}/url"),
                    true,
                ),
                api_key: connection.api_key.as_ref().map(|value| {
                    launch_value_source(
                        value,
                        &format!("connections/{connection_id}/apiKey"),
                        false,
                    )
                }),
            })
        })
        .transpose()?;

    let mut additional_paths = vec![AdditionalPathIdentity::SessionWorkspace {
        path: "/workspace".to_owned(),
    }];
    additional_paths.extend(session_mounts.iter().map(|mount| {
        AdditionalPathIdentity::MountedWorkspace {
            workspace_id: mount.workspace_id.clone(),
            mode: mount.mode,
            path: mount.mount_path(),
        }
    }));
    if let Some(source) = env_sources.get("COPILOT_ADDITIONAL_PATHS") {
        additional_paths.push(AdditionalPathIdentity::Configured {
            source: source.clone(),
        });
    }

    Ok(CliLaunchSnapshot {
        schema_version: 1,
        provider,
        model: env_sources.get("COPILOT_MODEL").cloned(),
        reasoning_effort: env_sources.get("COPILOT_REASONING_EFFORT").cloned(),
        options: CliLaunchOptionsSnapshot {
            no_auto_update: true,
            mouse: true,
            config_dir: env_sources.get("COPILOT_CONFIG_DIR").cloned(),
            extra_args: env_sources.get("COPILOT_EXTRA_ARGS").cloned(),
        },
        additional_paths,
        agent_profile: agent_profile_snapshot(config_agent, session_id),
    })
}

fn config_env_sources(
    env: Option<&BTreeMap<String, ConfigValue<String>>>,
    env_text: Option<&str>,
    resource_path: &str,
) -> BTreeMap<String, LaunchValueSource> {
    let mut sources = BTreeMap::new();
    if let Some(env_text) = env_text {
        for (key, value) in crate::models::parse_env_vars(env_text) {
            sources.insert(key, LaunchValueSource::Literal { value });
        }
    }
    if let Some(env) = env {
        for (key, value) in env {
            sources.insert(
                key.clone(),
                launch_value_source(value, &format!("{resource_path}/env/{key}"), true),
            );
        }
    }
    sources
}

fn launch_value_source(
    value: &ConfigValue<String>,
    field: &str,
    allow_literal: bool,
) -> LaunchValueSource {
    match value {
        ConfigValue::Literal(value) if allow_literal => LaunchValueSource::Literal {
            value: value.clone(),
        },
        ConfigValue::Literal(_) => LaunchValueSource::ConfigReference {
            field: field.to_owned(),
        },
        ConfigValue::Secret(name) => LaunchValueSource::SecretReference {
            field: field.to_owned(),
            name: name.clone(),
        },
    }
}

fn agent_profile_snapshot(
    agent: &ConfigAgent,
    session_id: &str,
) -> Option<AgentProfileLaunchSnapshot> {
    if matches!(&agent.system_prompt, ConfigValue::Literal(prompt) if prompt.is_empty()) {
        return None;
    }
    Some(AgentProfileLaunchSnapshot {
        identity: format!("agentspace-{session_id}"),
        system_prompt: launch_value_source(
            &agent.system_prompt,
            &format!("agents/{}/systemPrompt", agent.id),
            true,
        ),
    })
}

async fn list_sessions(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let sessions = state
        .sessions
        .list()?
        .into_iter()
        .map(|session| session_summary(&state, &session))
        .collect::<Result<Vec<_>, _>>()?;
    tracing::info!(
        route = "/sessions",
        action = "list_sessions",
        session_count = sessions.len(),
        "api handler completed"
    );
    Ok(Json(sessions))
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut session = require_session(&state, &session_id)?;
    if session.interaction_mode == InteractionMode::Cli {
        return Ok(Json(session_detail(&state, &session)?));
    }
    let upstream = state
        .agent_host
        .get_session(&session.agent_host_session_id)
        .await?;
    if let Ok(status) = string_field(&upstream, "status") {
        session.status = status;
        session.updated_at = utc_now();
        state.sessions.update(session.clone())?;
    }
    tracing::info!(
        route = "/sessions/:session_id",
        action = "get_session",
        session_id = %session_id,
        agent_id = %session.agent_id,
        kernel_session_id = %session.agent_host_session_id,
        status = %session.status,
        message_count = session.messages.len(),
        "api handler completed"
    );
    Ok(Json(session_detail(&state, &session)?))
}

async fn list_messages(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let session = require_chat_session(&state, &session_id)?;
    let messages = session
        .messages
        .iter()
        .map(MessageRecord::summary)
        .collect::<Vec<_>>();
    tracing::info!(
        route = "/sessions/:session_id/messages",
        action = "list_messages",
        session_id = %session_id,
        agent_id = %session.agent_id,
        kernel_session_id = %session.agent_host_session_id,
        message_count = messages.len(),
        "api handler completed"
    );
    Ok(Json(json!({ "messages": messages })))
}

async fn send_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<Json<Value>, ApiError> {
    tracing::info!(
        route = "/sessions/:session_id/messages",
        action = "send_message",
        session_id = %session_id,
        "starting synchronous turn"
    );
    Ok(Json(run_turn(&state, &session_id, &payload.message).await?))
}

async fn stream_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<Response, ApiError> {
    let (turn, receiver) = start_streaming_turn(&state, &session_id, payload.message)?;
    tracing::info!(
        route = "/sessions/:session_id/messages/stream",
        action = "stream_message",
        session_id = %turn.session_id,
        turn_id = %turn.turn_id,
        kernel_session_id = %turn.agent_host_session_id,
        "stream response started"
    );
    tokio::spawn(run_streaming_turn(state, turn));
    Ok(ndjson_stream_response(receiver))
}

async fn stream_turn(
    State(state): State<AppState>,
    Path((session_id, turn_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let session = require_chat_session(&state, &session_id)?;
    let receiver = subscribe_active_turn(&state, &session_id, &turn_id)?;
    tracing::info!(
        route = "/sessions/:session_id/turns/:turn_id/stream",
        action = "stream_turn",
        session_id = %session_id,
        turn_id = %turn_id,
        kernel_session_id = %session.agent_host_session_id,
        "turn stream attached"
    );
    Ok(ndjson_stream_response(receiver))
}

async fn reset_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut session = require_chat_session(&state, &session_id)?;
    let upstream = state
        .agent_host
        .reset_session(&session.agent_host_session_id)
        .await?;
    session.agent_host_session_id = string_field(&upstream, "session_id")?;
    session.status = string_field(&upstream, "status")?;
    session.updated_at = utc_now();
    state.sessions.clear_messages(&session_id)?;
    state.sessions.update(session.clone())?;
    tracing::info!(
        route = "/sessions/:session_id/reset",
        action = "reset_session",
        session_id = %session_id,
        agent_id = %session.agent_id,
        kernel_session_id = %session.agent_host_session_id,
        status = %session.status,
        "api handler completed"
    );
    Ok(Json(session_summary(&state, &session)?))
}

async fn save_session_workspace(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SaveSessionWorkspaceRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_workspace_id(&payload.workspace_id)?;
    let session = require_chat_session(&state, &session_id)?;
    let mut workspace = WorkspaceRecord::new_with_status(
        payload.workspace_id,
        payload.name,
        WorkspaceStatus::Creating,
    );
    let volume_name = workspace.volume_name();
    let workspace_id = workspace.workspace_id.clone();
    let mut exclude_names = state
        .agents
        .get(&session.agent_id)?
        .map(|agent| {
            agent
                .workspace_mounts
                .into_iter()
                .map(|mount| mount.workspace_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    exclude_names.push(".agents".to_owned());
    state.workspaces.insert(workspace.clone())?;
    let snapshot_result = state
        .agent_host
        .snapshot_session_workspace(
            &session.agent_host_session_id,
            &workspace_id,
            &volume_name,
            &exclude_names,
        )
        .await;
    if let Err(error) = snapshot_result {
        workspace.status = WorkspaceStatus::Failed;
        workspace.updated_at = utc_now();
        state.workspaces.update(workspace)?;
        return Err(error.into());
    }
    workspace.status = WorkspaceStatus::Ready;
    workspace.updated_at = utc_now();
    let value = workspace.summary();
    state.workspaces.update(workspace)?;
    tracing::info!(
        route = "/sessions/:session_id/workspace/save",
        action = "save_session_workspace",
        session_id = %session_id,
        workspace_id = %workspace_id,
        kernel_session_id = %session.agent_host_session_id,
        excluded_workspace_count = exclude_names.len(),
        "api handler completed"
    );
    Ok(Json(value))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let session = require_session(&state, &session_id)?;
    if !state.sessions.delete(&session_id)? {
        return Err(ApiError::not_found(format!(
            "session {session_id:?} not found"
        )));
    }
    if session.interaction_mode == InteractionMode::Chat {
        state
            .agent_host
            .destroy_session(&session.agent_host_session_id)
            .await?;
    }
    tracing::info!(
        route = "/sessions/:session_id",
        action = "delete_session",
        session_id = %session_id,
        agent_id = %session.agent_id,
        kernel_session_id = %session.agent_host_session_id,
        "api handler completed"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn list_kernels(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let upstream_sessions = state.agent_host.list_sessions(true).await?;
    let client_sessions = state.sessions.list()?;
    let mut kernels = Vec::new();
    for mut upstream in upstream_sessions {
        let upstream_session_id = string_field(&upstream, "session_id")?;
        let linked_sessions = client_sessions
            .iter()
            .filter(|session| session.agent_host_session_id == upstream_session_id)
            .collect::<Vec<_>>();
        let client_session_ids = linked_sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect::<Vec<_>>();
        let channel_names = linked_sessions
            .iter()
            .filter_map(|session| session.channel_name.clone())
            .collect::<Vec<_>>();
        let agent_ids = linked_sessions
            .iter()
            .map(|session| session.agent_id.clone())
            .collect::<Vec<_>>();
        upstream.insert("client_session_ids".to_owned(), json!(client_session_ids));
        upstream.insert("channel_names".to_owned(), json!(channel_names));
        upstream.insert("agent_ids".to_owned(), json!(agent_ids));
        kernels.push(Value::Object(upstream));
    }
    tracing::info!(
        route = "/kernels",
        action = "list_kernels",
        kernel_count = kernels.len(),
        client_session_count = client_sessions.len(),
        "api handler completed"
    );
    Ok(Json(kernels))
}

async fn kill_kernel(
    State(state): State<AppState>,
    Path(kernel_session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    require_kernel(&state, &kernel_session_id).await?;
    state.agent_host.destroy_session(&kernel_session_id).await?;
    for mut session in state.sessions.list()? {
        if session.agent_host_session_id == kernel_session_id {
            "dead".clone_into(&mut session.status);
            session.updated_at = utc_now();
            state.sessions.update(session)?;
        }
    }
    tracing::info!(
        route = "/kernels/:kernel_session_id",
        action = "kill_kernel",
        kernel_session_id = %kernel_session_id,
        "api handler completed"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn kernel_logs(
    State(state): State<AppState>,
    Path(kernel_session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_kernel(&state, &kernel_session_id).await?;
    let lines = state.agent_host.logs(&kernel_session_id).await?;
    tracing::info!(
        route = "/kernels/:kernel_session_id/logs",
        action = "kernel_logs",
        kernel_session_id = %kernel_session_id,
        line_count = lines.len(),
        "api handler completed"
    );
    Ok(Json(json!({ "lines": lines })))
}

async fn kernel_container_logs(
    State(state): State<AppState>,
    Path(kernel_session_id): Path<String>,
    Query(query): Query<ContainerLogsQuery>,
) -> Result<Json<Value>, ApiError> {
    require_kernel(&state, &kernel_session_id).await?;
    let tail = if query.all.unwrap_or(false) {
        None
    } else {
        Some(query.tail.unwrap_or(2_000))
    };
    if let Some(tail) = tail
        && !(1..=50_000).contains(&tail)
    {
        return Err(ApiError::unprocessable(
            "tail must be between 1 and 50000".to_owned(),
        ));
    }
    let lines = state
        .agent_host
        .container_logs(&kernel_session_id, tail)
        .await?;
    tracing::info!(
        route = "/kernels/:kernel_session_id/container-logs",
        action = "kernel_container_logs",
        kernel_session_id = %kernel_session_id,
        tail = ?tail,
        line_count = lines.len(),
        "api handler completed"
    );
    Ok(Json(json!({ "lines": lines })))
}

async fn create_skill(
    State(state): State<AppState>,
    Json(payload): Json<CreateSkillRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_skill_id(&payload.skill_id)?;
    // Serialize with /config/apply and every other skill mutation so upstream
    // staging and document commits cannot interleave into a host/document
    // split-brain.
    let _apply_guard = state.apply_lock.lock().await;
    // A user skill must not collide with an installation-owned builtin id.
    let builtins = builtin_skill_ids(&state).await;
    if builtins.contains(&payload.skill_id) {
        return Err(ApiError::conflict(format!(
            "skill id {:?} collides with an installation-owned builtin skill",
            payload.skill_id
        )));
    }
    if let Some(creator_agent_id) = payload.creator_agent_id.as_deref() {
        validate_agent_id(creator_agent_id)?;
        require_agent(&state, creator_agent_id)?;
    }
    // Stage the upstream skill first so the document is never committed while
    // agent_host is stale. If the document commit fails, compensate upstream.
    let skill = state
        .agent_host
        .create_skill(&payload.skill_id, &payload.files)
        .await?;
    if let Err(error) = config::adapter::upsert_skill(
        &state.config_state,
        &payload.skill_id,
        payload.files.clone(),
    ) {
        let _ = state.agent_host.delete_skill(&payload.skill_id).await;
        return Err(error.into());
    }
    let auto_enabled = payload
        .creator_agent_id
        .as_deref()
        .map(|agent_id| state.agents.add_skill(agent_id, &payload.skill_id))
        .transpose()?
        .unwrap_or(false);
    tracing::info!(
        route = "/skills",
        action = "create_skill",
        skill_id = %payload.skill_id,
        file_count = payload.files.len(),
        creator_agent_id = payload.creator_agent_id.as_deref().unwrap_or_default(),
        auto_enabled,
        "api handler completed"
    );
    Ok(Json(Value::Object(skill)))
}

async fn list_skills(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    // User skills are authoritative from the ConfigDocument; only builtins are
    // sourced from agent_host. Runtime agent_host user-skill state is never a
    // read/export source of truth.
    let mut skills: Vec<Value> = config::adapter::list_skills(&state.config_state)?
        .into_iter()
        .map(|skill| {
            json!({
                "skill_id": skill.id,
                "source": "user",
                "files": skill.files,
            })
        })
        .collect();
    match state.agent_host.list_skills().await {
        Ok(entries) => {
            for entry in entries {
                let is_builtin = entry
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| source == "builtin");
                if is_builtin {
                    skills.push(Value::Object(entry));
                }
            }
        }
        Err(error) => {
            tracing::warn!(
                route = "/skills",
                action = "list_skills",
                error_kind = "agent_host_error",
                error = %error,
                "could not list builtin skills from agent_host; returning user skills only"
            );
        }
    }
    // User skills are listed first in document order, then installation-owned
    // builtins; document skills are already deterministically ordered.
    tracing::info!(
        route = "/skills",
        action = "list_skills",
        skill_count = skills.len(),
        "api handler completed"
    );
    Ok(Json(skills))
}

async fn get_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // Authored user skills are served from the document; anything else is a
    // builtin proxied from agent_host.
    if let Some(skill) = config::adapter::get_skill(&state.config_state, &skill_id)? {
        tracing::info!(
            route = "/skills/:skill_id",
            action = "get_skill",
            skill_id = %skill_id,
            source = "user",
            "api handler completed"
        );
        return Ok(Json(json!({
            "skill_id": skill.id,
            "source": "user",
            "files": skill.files,
        })));
    }
    let skill = state.agent_host.get_skill(&skill_id).await?;
    tracing::info!(
        route = "/skills/:skill_id",
        action = "get_skill",
        skill_id = %skill_id,
        source = "builtin",
        "api handler completed"
    );
    Ok(Json(Value::Object(skill)))
}

async fn download_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<Response, ApiError> {
    validate_skill_id(&skill_id)?;
    // Authored user skills download from the authoritative document contents,
    // never from potentially stale agent_host runtime state.
    if let Some(skill) = config::adapter::get_skill(&state.config_state, &skill_id)? {
        let body = zip_skill_files(&skill.files)?;
        let mut response = Body::from(body).into_response();
        insert_download_header(
            response.headers_mut(),
            header::CONTENT_TYPE,
            "application/zip",
        )?;
        insert_download_header(
            response.headers_mut(),
            header::CONTENT_DISPOSITION,
            &format!("attachment; filename=\"{skill_id}.zip\""),
        )?;
        tracing::info!(
            route = "/skills/:skill_id/download",
            action = "download_skill",
            skill_id = %skill_id,
            source = "user",
            "api handler completed"
        );
        return Ok(response);
    }
    let download = state.agent_host.download_skill(&skill_id).await?;
    let mut response = Body::from(download.body).into_response();
    insert_download_header(
        response.headers_mut(),
        header::CONTENT_TYPE,
        &download.content_type,
    )?;
    insert_download_header(
        response.headers_mut(),
        header::CONTENT_DISPOSITION,
        &download.content_disposition,
    )?;
    tracing::info!(
        route = "/skills/:skill_id/download",
        action = "download_skill",
        skill_id = %skill_id,
        source = "builtin",
        "api handler completed"
    );
    Ok(response)
}

async fn list_skill_versions(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let versions = state
        .agent_host
        .list_skill_versions(&skill_id)
        .await?
        .into_iter()
        .map(Value::Object)
        .collect::<Vec<_>>();
    tracing::info!(
        route = "/skills/:skill_id/versions",
        action = "list_skill_versions",
        skill_id = %skill_id,
        version_count = versions.len(),
        "api handler completed"
    );
    Ok(Json(versions))
}

async fn update_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
    Json(payload): Json<UpdateSkillRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_skill_id(&skill_id)?;
    // Serialize with /config/apply and every other skill mutation.
    let _apply_guard = state.apply_lock.lock().await;
    // Capture the previously authored files so the upstream skill can be
    // restored if the document commit fails after a successful upstream update.
    let previous =
        config::adapter::get_skill(&state.config_state, &skill_id)?.map(|skill| skill.files);
    let skill = state
        .agent_host
        .update_skill(&skill_id, &payload.files)
        .await?;
    if let Err(error) =
        config::adapter::upsert_skill(&state.config_state, &skill_id, payload.files.clone())
    {
        if let Some(previous) = previous {
            let _ = state.agent_host.update_skill(&skill_id, &previous).await;
        }
        return Err(error.into());
    }
    tracing::info!(
        route = "/skills/:skill_id",
        action = "update_skill",
        skill_id = %skill_id,
        file_count = payload.files.len(),
        "api handler completed"
    );
    Ok(Json(Value::Object(skill)))
}

async fn rollback_skill_version(
    State(state): State<AppState>,
    Path((skill_id, version)): Path<(String, u64)>,
) -> Result<Json<Value>, ApiError> {
    validate_skill_id(&skill_id)?;
    // Serialize with /config/apply and every other skill mutation.
    let _apply_guard = state.apply_lock.lock().await;
    // Capture the currently authored files so the upstream skill can be restored
    // if the document commit fails after a successful upstream rollback.
    let previous =
        config::adapter::get_skill(&state.config_state, &skill_id)?.map(|skill| skill.files);
    let skill = state
        .agent_host
        .rollback_skill_version(&skill_id, version)
        .await?;
    // Keep the authoritative document in sync with the rolled-back files, but
    // only for skills already authored as user skills (never author builtins).
    if previous.is_some() {
        let files = skill_files_from_object(&skill);
        if let Err(error) = config::adapter::upsert_skill(&state.config_state, &skill_id, files) {
            if let Some(previous) = previous {
                let _ = state.agent_host.update_skill(&skill_id, &previous).await;
            }
            return Err(error.into());
        }
    }
    tracing::info!(
        route = "/skills/:skill_id/versions/:version/rollback",
        action = "rollback_skill_version",
        skill_id = %skill_id,
        version,
        "api handler completed"
    );
    Ok(Json(Value::Object(skill)))
}

async fn delete_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    validate_skill_id(&skill_id)?;
    // Serialize with /config/apply and every other skill mutation.
    let _apply_guard = state.apply_lock.lock().await;
    // A skill referenced by any authored agent cannot be deleted; the reference
    // would dangle in the desired document.
    if let Some(agent_id) = agent_referencing_skill(&state, &skill_id) {
        return Err(ApiError::conflict(format!(
            "skill {skill_id:?} is referenced by agent {agent_id:?} and cannot be deleted"
        )));
    }
    // Capture files so the upstream skill can be recreated if the document
    // commit fails after a successful upstream delete.
    let previous =
        config::adapter::get_skill(&state.config_state, &skill_id)?.map(|skill| skill.files);
    state.agent_host.delete_skill(&skill_id).await?;
    if let Err(error) = config::adapter::delete_skill(&state.config_state, &skill_id) {
        if let Some(previous) = previous {
            let _ = state.agent_host.create_skill(&skill_id, &previous).await;
        }
        return Err(error.into());
    }
    tracing::info!(
        route = "/skills/:skill_id",
        action = "delete_skill",
        skill_id = %skill_id,
        "api handler completed"
    );
    Ok(StatusCode::NO_CONTENT)
}

fn insert_download_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: &str,
) -> Result<(), ApiError> {
    let value = HeaderValue::from_str(value).map_err(|source| {
        ApiError::bad_gateway(format!(
            "agent_host returned invalid download header {}: {source}",
            name.as_str()
        ))
    })?;
    headers.insert(name, value);
    Ok(())
}

async fn list_gateway_types() -> Json<Vec<&'static str>> {
    let gateway_types = GatewayType::all()
        .iter()
        .map(|gateway_type| gateway_type.as_str())
        .collect::<Vec<_>>();
    tracing::info!(
        route = "/gateway-types",
        action = "list_gateway_types",
        gateway_type_count = gateway_types.len(),
        "api handler completed"
    );
    Json(gateway_types)
}

async fn get_gateway_type_schema(
    Path(raw_gateway_type): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let gateway_type = parse_gateway_type(&raw_gateway_type)?;
    tracing::info!(
        route = "/gateway-types/:gateway_type/schema",
        action = "get_gateway_type_schema",
        gateway_type = gateway_type.as_str(),
        "api handler completed"
    );
    Ok(Json(gateway_type_schema(gateway_type)))
}

fn gateway_type_schema(gateway_type: GatewayType) -> Value {
    match gateway_type {
        GatewayType::Echo => json!({ "fields": [] }),
        GatewayType::Discord => json!({
            "fields": [
                {
                    "key": "DISCORD_BOT_TOKEN",
                    "label": "Bot token",
                    "kind": "secret",
                    "required": true,
                    "description": "Bot token from the Discord Developer Portal.",
                },
                {
                    "key": "DISCORD_OWNER_USER_ID",
                    "label": "Owner user ID",
                    "kind": "env",
                    "required": true,
                    "description": "Discord snowflake user ID of the only user the bot will respond to in DMs.",
                    "placeholder": "123456789012345678",
                },
                {
                    "key": "DISCORD_CHUNK_MAX_CHARS",
                    "label": "Max chunk size (chars)",
                    "kind": "env",
                    "required": false,
                    "description": "Maximum characters per outbound Discord message. Discord's hard limit is 2000.",
                    "default": "1900",
                },
                {
                    "key": "DISCORD_SIMULATED_TYPING_ENABLED",
                    "label": "Simulated typing enabled",
                    "kind": "env",
                    "required": false,
                    "description": "If true, deliver the agent reply as multiple messages (split on paragraph boundaries) with a typing indicator and a per-paragraph delay sized by SIMULATED_TYPING_WPM. Makes responses feel human-paced.",
                    "default": "false",
                },
                {
                    "key": "DISCORD_SIMULATED_TYPING_WPM",
                    "label": "Simulated typing speed (wpm)",
                    "kind": "env",
                    "required": false,
                    "description": "Words-per-minute used to size simulated typing delays. Ignored when SIMULATED_TYPING_ENABLED is false.",
                    "default": "220",
                },
            ],
        }),
    }
}

async fn list_gateways(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let gateways = state
        .gateways
        .list()?
        .into_iter()
        .map(|gateway| gateway.summary(false))
        .collect::<Vec<_>>();
    tracing::info!(
        route = "/gateways",
        action = "list_gateways",
        gateway_count = gateways.len(),
        "api handler completed"
    );
    Ok(Json(gateways))
}

async fn create_gateway(
    State(state): State<AppState>,
    Json(payload): Json<CreateGatewayRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_gateway_id(&payload.gateway_id)?;
    require_agent(&state, &payload.agent_id)?;
    let agent_id = payload.agent_id.clone();
    let gateway_type = payload.gateway_type;
    let enabled = payload.enabled;
    let mut gateway = GatewayRecord::new(
        payload.gateway_id,
        payload.name,
        payload.gateway_type,
        payload.agent_id,
        payload.enabled,
    );
    gateway.env_vars = payload.env_vars;
    gateway.secrets = payload.secrets;
    let gateway_id = gateway.gateway_id.clone();
    state.gateways.insert(&gateway)?;
    tracing::info!(
        route = "/gateways",
        action = "create_gateway",
        gateway_id = %gateway_id,
        agent_id = %agent_id,
        gateway_type = gateway_type.as_str(),
        enabled,
        "gateway created"
    );
    if enabled {
        return start_gateway_by_id(&state, &gateway_id, GatewayStartFailureMode::ReturnRecord)
            .await
            .map(Json);
    }
    let gateway = require_gateway(&state, &gateway_id)?;
    tracing::info!(
        route = "/gateways",
        action = "create_gateway",
        gateway_id = %gateway_id,
        agent_id = %gateway.agent_id,
        gateway_type = gateway.gateway_type.as_str(),
        status = %gateway.status,
        "api handler completed"
    );
    Ok(Json(gateway.summary(false)))
}

async fn get_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let gateway = require_gateway(&state, &gateway_id)?;
    tracing::info!(
        route = "/gateways/:gateway_id",
        action = "get_gateway",
        gateway_id = %gateway_id,
        agent_id = %gateway.agent_id,
        gateway_type = gateway.gateway_type.as_str(),
        enabled = gateway.enabled,
        status = %gateway.status,
        "api handler completed"
    );
    Ok(Json(gateway.summary(false)))
}

async fn update_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
    Json(payload): Json<UpdateGatewayRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut gateway = require_gateway(&state, &gateway_id)?;
    let was_running = gateway.status == "running";
    let previously_enabled = gateway.enabled;
    let mut config_changed = false;
    if let Some(name) = payload.name {
        gateway.name = name;
    }
    if let Some(agent_id) = payload.agent_id
        && agent_id != gateway.agent_id
    {
        require_agent(&state, &agent_id)?;
        gateway.agent_id = agent_id;
        config_changed = true;
    }
    if let Some(enabled) = payload.enabled {
        gateway.enabled = enabled;
    }
    if let Some(env_vars) = payload.env_vars
        && env_vars != gateway.env_vars
    {
        gateway.env_vars = env_vars;
        config_changed = true;
    }
    if let Some(secrets) = payload.secrets {
        let mut merged = gateway.secrets.clone();
        merged.extend(secrets);
        if merged != gateway.secrets {
            gateway.secrets = merged;
            config_changed = true;
        }
    }
    gateway.updated_at = utc_now();
    let enabled = gateway.enabled;
    let gateway_type = gateway.gateway_type;
    let agent_id = gateway.agent_id.clone();
    state.gateways.update(&gateway)?;
    tracing::info!(
        route = "/gateways/:gateway_id",
        action = "update_gateway",
        gateway_id = %gateway_id,
        agent_id = %agent_id,
        gateway_type = gateway_type.as_str(),
        enabled,
        previously_enabled,
        config_changed,
        was_running,
        "gateway updated"
    );
    if enabled && !previously_enabled {
        start_gateway_by_id(&state, &gateway_id, GatewayStartFailureMode::ReturnRecord)
            .await
            .map(Json)
    } else if !enabled && previously_enabled {
        stop_gateway_by_id(&state, &gateway_id).await.map(Json)
    } else if config_changed && was_running {
        let _stopped = stop_gateway_by_id(&state, &gateway_id).await?;
        start_gateway_by_id(&state, &gateway_id, GatewayStartFailureMode::ReturnRecord)
            .await
            .map(Json)
    } else {
        let gateway = require_gateway(&state, &gateway_id)?;
        Ok(Json(gateway.summary(false)))
    }
}

async fn delete_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let gateway = require_gateway(&state, &gateway_id)?;
    if !matches!(gateway.status.as_str(), "stopped" | "error") {
        tracing::info!(
            route = "/gateways/:gateway_id",
            action = "delete_gateway",
            gateway_id = %gateway_id,
            agent_id = %gateway.agent_id,
            gateway_type = gateway.gateway_type.as_str(),
            status = %gateway.status,
            "destroying gateway before delete"
        );
        let _ignored = state.agent_host.destroy_gateway(&gateway_id).await;
    }
    let _removed = state.gateways.delete(&gateway_id)?;
    tracing::info!(
        route = "/gateways/:gateway_id",
        action = "delete_gateway",
        gateway_id = %gateway_id,
        "api handler completed"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn start_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    tracing::info!(
        route = "/gateways/:gateway_id/start",
        action = "start_gateway",
        gateway_id = %gateway_id,
        "starting gateway"
    );
    start_gateway_by_id(&state, &gateway_id, GatewayStartFailureMode::Propagate)
        .await
        .map(Json)
}

async fn stop_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    tracing::info!(
        route = "/gateways/:gateway_id/stop",
        action = "stop_gateway",
        gateway_id = %gateway_id,
        "stopping gateway"
    );
    stop_gateway_by_id(&state, &gateway_id).await.map(Json)
}

async fn gateway_logs(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _gateway = require_gateway(&state, &gateway_id)?;
    let lines = state.agent_host.gateway_logs(&gateway_id).await?;
    tracing::info!(
        route = "/gateways/:gateway_id/logs",
        action = "gateway_logs",
        gateway_id = %gateway_id,
        line_count = lines.len(),
        "api handler completed"
    );
    Ok(Json(json!({ "lines": lines })))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GatewayStartFailureMode {
    Propagate,
    ReturnRecord,
}

pub async fn start_enabled_gateways(state: AppState) {
    let gateways = match state.gateways.list() {
        Ok(gateways) => gateways,
        Err(error) => {
            tracing::error!(
                action = "start_enabled_gateways",
                error = %error,
                "failed to list gateways for startup"
            );
            return;
        }
    };
    let enabled_gateways = gateways
        .into_iter()
        .filter(|gateway| gateway.enabled)
        .collect::<Vec<_>>();
    if enabled_gateways.is_empty() {
        tracing::info!(
            action = "start_enabled_gateways",
            gateway_count = 0,
            "no enabled gateways to start"
        );
        return;
    }

    tracing::info!(
        action = "start_enabled_gateways",
        gateway_count = enabled_gateways.len(),
        "starting enabled gateways"
    );
    for gateway in enabled_gateways {
        start_enabled_gateway_with_retries(&state, &gateway.gateway_id).await;
    }
}

async fn start_enabled_gateway_with_retries(state: &AppState, gateway_id: &str) {
    for attempt in 1..=GATEWAY_AUTOSTART_ATTEMPTS {
        match start_gateway_by_id(state, gateway_id, GatewayStartFailureMode::Propagate).await {
            Ok(_gateway) => {
                tracing::info!(
                    action = "start_enabled_gateways",
                    gateway_id = %gateway_id,
                    attempt,
                    "enabled gateway started"
                );
                return;
            }
            Err(error) if attempt < GATEWAY_AUTOSTART_ATTEMPTS => {
                tracing::warn!(
                    action = "start_enabled_gateways",
                    gateway_id = %gateway_id,
                    attempt,
                    status = error.status.as_u16(),
                    error = %error.detail,
                    retry_delay_ms = GATEWAY_AUTOSTART_RETRY_DELAY.as_millis(),
                    "enabled gateway start failed; retrying"
                );
                sleep(GATEWAY_AUTOSTART_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::warn!(
                    action = "start_enabled_gateways",
                    gateway_id = %gateway_id,
                    attempt,
                    status = error.status.as_u16(),
                    error = %error.detail,
                    "enabled gateway start failed"
                );
                return;
            }
        }
    }
}

/// Reconcile every gateway container to the desired `ConfigDocument` on startup.
///
/// Unlike [`start_enabled_gateways`] (which only starts enabled gateways), this
/// performs a complete reconcile against the state observed in `agent_host`:
/// enabled gateways are started (with retries), disabled-but-running gateways
/// are stopped, and upstream gateways that are absent from the desired document
/// (orphans) are destroyed (with retries). It runs under the apply lock so it
/// never races a concurrent apply or interactive gateway mutation.
pub async fn reconcile_gateways_on_startup(state: AppState) {
    let _guard = state.apply_lock.lock().await;
    reconcile_gateways_locked(&state).await;
}

async fn reconcile_gateways_locked(state: &AppState) {
    let desired = match state.gateways.list() {
        Ok(gateways) => gateways,
        Err(error) => {
            tracing::error!(
                action = "reconcile_gateways_on_startup",
                error = %error,
                "failed to list desired gateways for startup reconcile"
            );
            return;
        }
    };
    let observed_ids = match state.agent_host.list_gateways().await {
        Ok(list) => list
            .iter()
            .filter_map(|gateway| gateway.get("gateway_id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<String>>(),
        Err(error) => {
            // The observed state is unknown, so we cannot safely stop or destroy
            // anything. Still start enabled gateways (idempotent) and log so the
            // orphan/stop reconcile can be retried on the next startup or apply.
            tracing::error!(
                action = "reconcile_gateways_on_startup",
                error = %error,
                "failed to list agent_host gateways; only starting enabled gateways"
            );
            for gateway in desired.iter().filter(|gateway| gateway.enabled) {
                start_enabled_gateway_with_retries(state, &gateway.gateway_id).await;
            }
            return;
        }
    };

    let desired_ids: BTreeSet<&str> = desired
        .iter()
        .map(|gateway| gateway.gateway_id.as_str())
        .collect();

    for gateway in &desired {
        if gateway.enabled {
            start_enabled_gateway_with_retries(state, &gateway.gateway_id).await;
        } else if observed_ids.contains(&gateway.gateway_id) {
            tracing::info!(
                action = "reconcile_gateways_on_startup",
                gateway_id = %gateway.gateway_id,
                "stopping disabled gateway that is still running"
            );
            if let Err(error) = stop_gateway_by_id(state, &gateway.gateway_id).await {
                tracing::warn!(
                    action = "reconcile_gateways_on_startup",
                    gateway_id = %gateway.gateway_id,
                    error = %error.detail,
                    "failed to stop disabled gateway during startup reconcile"
                );
            }
        }
    }

    for id in &observed_ids {
        if desired_ids.contains(id.as_str()) {
            continue;
        }
        destroy_orphan_gateway_with_retries(state, id).await;
    }
}

async fn destroy_orphan_gateway_with_retries(state: &AppState, gateway_id: &str) {
    for attempt in 1..=GATEWAY_AUTOSTART_ATTEMPTS {
        match state.agent_host.destroy_gateway(gateway_id).await {
            Ok(()) => {
                tracing::info!(
                    action = "reconcile_gateways_on_startup",
                    gateway_id = %gateway_id,
                    attempt,
                    "destroyed orphaned gateway not present in desired config"
                );
                return;
            }
            Err(error) if attempt < GATEWAY_AUTOSTART_ATTEMPTS => {
                tracing::warn!(
                    action = "reconcile_gateways_on_startup",
                    gateway_id = %gateway_id,
                    attempt,
                    error = %error,
                    retry_delay_ms = GATEWAY_AUTOSTART_RETRY_DELAY.as_millis(),
                    "failed to destroy orphaned gateway; retrying"
                );
                sleep(GATEWAY_AUTOSTART_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::warn!(
                    action = "reconcile_gateways_on_startup",
                    gateway_id = %gateway_id,
                    attempt,
                    error = %error,
                    "failed to destroy orphaned gateway"
                );
                return;
            }
        }
    }
}

/// Reconcile `agent_host` user skills to the active `ConfigDocument` on startup.
///
/// The active document is authoritative: every declared user skill is created
/// or updated in `agent_host` (unchanged skills are compared and skipped) and
/// user skills present upstream but absent from the document are removed. If the
/// upstream skill state cannot be determined the reconcile is retried and, if it
/// still fails, an error is logged (nothing is committed because the document is
/// already the source of truth). Runs under the apply lock so it never races a
/// concurrent apply or interactive skill mutation.
pub async fn reconcile_skills_on_startup(state: AppState) {
    let _guard = state.apply_lock.lock().await;
    reconcile_skills_locked(&state).await;
}

async fn reconcile_skills_locked(state: &AppState) {
    let document = state.config_state.active();
    let Some(host_user) = agent_host_user_skills_with_retries(state).await else {
        tracing::error!(
            action = "reconcile_skills_on_startup",
            "could not determine agent_host user-skill state; skipping startup skill \
             reconcile (will retry on next apply)"
        );
        return;
    };

    let mut performed: Vec<StagedSkillOp> = Vec::new();
    for skill in &document.spec.skills {
        let result = match host_user.get(&skill.id) {
            None => state
                .agent_host
                .create_skill(&skill.id, &skill.files)
                .await
                .map(|_| Some(StagedSkillOp::Created(skill.id.clone()))),
            Some(existing) if existing != &skill.files => state
                .agent_host
                .update_skill(&skill.id, &skill.files)
                .await
                .map(|_| Some(StagedSkillOp::Updated(skill.id.clone(), existing.clone()))),
            Some(_) => Ok(None),
        };
        match result {
            Ok(Some(op)) => performed.push(op),
            Ok(None) => {}
            Err(error) => {
                tracing::error!(
                    action = "reconcile_skills_on_startup",
                    skill_id = %skill.id,
                    error = %error,
                    "failed to materialize skill during startup reconcile; compensating"
                );
                compensate_skills(state, &performed).await;
                return;
            }
        }
    }

    let desired: BTreeSet<&str> = document
        .spec
        .skills
        .iter()
        .map(|skill| skill.id.as_str())
        .collect();
    for (id, files) in &host_user {
        if desired.contains(id.as_str()) {
            continue;
        }
        if let Err(error) = state.agent_host.delete_skill(id).await {
            tracing::error!(
                action = "reconcile_skills_on_startup",
                skill_id = %id,
                error = %error,
                "failed to remove stale skill during startup reconcile; compensating"
            );
            compensate_skills(state, &performed).await;
            return;
        }
        performed.push(StagedSkillOp::Deleted(id.clone(), files.clone()));
    }

    tracing::info!(
        action = "reconcile_skills_on_startup",
        reconciled = performed.len(),
        "startup skill reconcile complete"
    );
}

async fn agent_host_user_skills_with_retries(
    state: &AppState,
) -> Option<BTreeMap<String, BTreeMap<String, String>>> {
    for attempt in 1..=GATEWAY_AUTOSTART_ATTEMPTS {
        match agent_host_user_skills(state).await {
            Ok(map) => return Some(map),
            Err(error) if attempt < GATEWAY_AUTOSTART_ATTEMPTS => {
                tracing::warn!(
                    action = "reconcile_skills_on_startup",
                    attempt,
                    error = %error.detail,
                    retry_delay_ms = GATEWAY_AUTOSTART_RETRY_DELAY.as_millis(),
                    "could not determine agent_host user-skill state; retrying"
                );
                sleep(GATEWAY_AUTOSTART_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::warn!(
                    action = "reconcile_skills_on_startup",
                    attempt,
                    error = %error.detail,
                    "could not determine agent_host user-skill state"
                );
                return None;
            }
        }
    }
    None
}

/// Reconcile skills and gateways to the desired `ConfigDocument` on startup.
///
/// Both reconcilers run under a single acquisition of the apply lock so they
/// never race a concurrent apply or interactive mutation.
pub async fn reconcile_on_startup(state: AppState) {
    let _guard = state.apply_lock.lock().await;
    reconcile_skills_locked(&state).await;
    reconcile_gateways_locked(&state).await;
}

async fn start_gateway_by_id(
    state: &AppState,
    gateway_id: &str,
    failure_mode: GatewayStartFailureMode,
) -> Result<Value, ApiError> {
    let mut gateway = require_gateway(state, gateway_id)?;
    let env = config::resolver::resolve_gateway_env(&state.config_state, gateway_id)
        .map_err(resolve_error_to_api)?;
    "starting".clone_into(&mut gateway.status);
    gateway.last_error = None;
    state.gateways.set_runtime_status(
        gateway_id,
        &gateway.status,
        gateway.last_error.clone(),
        gateway.container_name.clone(),
    );
    tracing::info!(
        action = "start_gateway_by_id",
        gateway_id = %gateway_id,
        agent_id = %gateway.agent_id,
        gateway_type = gateway.gateway_type.as_str(),
        env_var_count = env.len(),
        "creating upstream gateway"
    );
    match create_agent_host_gateway(state, &gateway, &env).await {
        Ok(response) => {
            apply_gateway_start_response(&mut gateway, &response);
            state.gateways.set_runtime_status(
                gateway_id,
                &gateway.status,
                gateway.last_error.clone(),
                gateway.container_name.clone(),
            );
            tracing::info!(
                action = "start_gateway_by_id",
                gateway_id = %gateway_id,
                agent_id = %gateway.agent_id,
                gateway_type = gateway.gateway_type.as_str(),
                status = %gateway.status,
                has_container = gateway.container_name.is_some(),
                "gateway started"
            );
            Ok(gateway.summary(false))
        }
        Err(error) => {
            let detail = error.to_string();
            "error".clone_into(&mut gateway.status);
            gateway.last_error = Some(detail);
            state.gateways.set_runtime_status(
                gateway_id,
                &gateway.status,
                gateway.last_error.clone(),
                gateway.container_name.clone(),
            );
            tracing::warn!(
                action = "start_gateway_by_id",
                gateway_id = %gateway_id,
                error_kind = "agent_host_error",
                "gateway start failed"
            );
            if failure_mode == GatewayStartFailureMode::Propagate {
                Err(error.into())
            } else {
                Ok(gateway.summary(false))
            }
        }
    }
}

async fn create_agent_host_gateway(
    state: &AppState,
    gateway: &GatewayRecord,
    env: &BTreeMap<String, String>,
) -> Result<JsonObject, AgentHostError> {
    let result = state
        .agent_host
        .create_gateway(
            &gateway.gateway_id,
            gateway.gateway_type.as_str(),
            &gateway.agent_id,
            env,
        )
        .await;
    match result {
        Err(AgentHostError::HttpStatus { status, .. }) if status == StatusCode::CONFLICT => {
            tracing::warn!(
                action = "start_gateway_by_id",
                gateway_id = %gateway.gateway_id,
                "upstream gateway already exists; replacing stale runtime"
            );
            state
                .agent_host
                .destroy_gateway(&gateway.gateway_id)
                .await?;
            state
                .agent_host
                .create_gateway(
                    &gateway.gateway_id,
                    gateway.gateway_type.as_str(),
                    &gateway.agent_id,
                    env,
                )
                .await
        }
        other => other,
    }
}

fn apply_gateway_start_response(gateway: &mut GatewayRecord, response: &JsonObject) {
    response
        .get("status")
        .and_then(Value::as_str)
        .filter(|status| is_gateway_status(status))
        .unwrap_or("running")
        .clone_into(&mut gateway.status);
    gateway.last_error = response
        .get("last_error")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    gateway.container_name = response
        .get("container_name")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
}

fn is_gateway_status(status: &str) -> bool {
    matches!(status, "stopped" | "starting" | "running" | "error")
}

async fn stop_gateway_by_id(state: &AppState, gateway_id: &str) -> Result<Value, ApiError> {
    let mut gateway = require_gateway(state, gateway_id)?;
    if let Err(error) = state.agent_host.destroy_gateway(gateway_id).await {
        gateway.last_error = Some(error.to_string());
        tracing::warn!(
            action = "stop_gateway_by_id",
            gateway_id = %gateway_id,
            error_kind = "agent_host_error",
            "gateway destroy failed while stopping"
        );
    }
    "stopped".clone_into(&mut gateway.status);
    gateway.container_name = None;
    state.gateways.set_runtime_status(
        gateway_id,
        &gateway.status,
        gateway.last_error.clone(),
        None,
    );
    tracing::info!(
        action = "stop_gateway_by_id",
        gateway_id = %gateway_id,
        agent_id = %gateway.agent_id,
        gateway_type = gateway.gateway_type.as_str(),
        status = %gateway.status,
        "gateway stopped"
    );
    Ok(gateway.summary(false))
}

async fn require_kernel(state: &AppState, kernel_session_id: &str) -> Result<(), ApiError> {
    let sessions = state.agent_host.list_sessions(false).await?;
    if sessions.iter().any(|session| {
        session
            .get("session_id")
            .and_then(Value::as_str)
            .is_some_and(|value| value == kernel_session_id)
    }) {
        tracing::debug!(
            action = "require_kernel",
            kernel_session_id = %kernel_session_id,
            kernel_count = sessions.len(),
            found = true,
            "kernel found"
        );
        Ok(())
    } else {
        tracing::warn!(
            action = "require_kernel",
            kernel_session_id = %kernel_session_id,
            kernel_count = sessions.len(),
            found = false,
            "kernel not found"
        );
        Err(ApiError::not_found(format!(
            "kernel {kernel_session_id:?} not found"
        )))
    }
}

fn session_env(
    state: &AppState,
    agent: &AgentRecord,
) -> Result<BTreeMap<String, String>, ApiError> {
    let mut missing = Vec::new();
    let mut env = BTreeMap::new();
    env.extend(
        config::resolver::resolve_kernel_env(&state.config_state, agent.harness, &mut missing)
            .map_err(resolve_error_to_api)?,
    );
    if let Some(connection_id) = agent.connection_id.as_deref() {
        let connection = require_connection(state, connection_id)?;
        let resolved =
            config::resolver::resolve_connection(&state.config_state, connection_id, &mut missing)
                .map_err(resolve_error_to_api)?;
        if let Some(url) = resolved.url {
            env.insert("CONNECTION_URL".to_owned(), url);
        }
        env.insert(
            "CONNECTION_API_FLAVOR".to_owned(),
            connection.api_flavor.as_str().to_owned(),
        );
        if let Some(api_key) = resolved.api_key {
            env.insert("CONNECTION_API_KEY".to_owned(), api_key);
        }
    }
    env.extend(
        config::resolver::resolve_agent_env(&state.config_state, &agent.agent_id, &mut missing)
            .map_err(resolve_error_to_api)?,
    );
    env.insert("AGENTSPACE_AGENT_ID".to_owned(), agent.agent_id.clone());
    env.insert(
        "AGENTSPACE_CLIENT_SERVICE_URL".to_owned(),
        state
            .config
            .client_service_env
            .get(AGENTSPACE_CLIENT_SERVICE_URL_ENV)
            .cloned()
            .unwrap_or_else(|| DEFAULT_AGENTSPACE_CLIENT_SERVICE_URL.to_owned()),
    );
    let system_prompt = config::resolver::resolve_agent_system_prompt(
        &state.config_state,
        &agent.agent_id,
        &mut missing,
    )
    .map_err(resolve_error_to_api)?
    // Fall back to the record's literal prompt for installation-owned agents
    // (e.g. the synthesized Git Agent reviewer) that are not authored into the
    // desired document.
    .or_else(|| {
        if agent.system_prompt.is_empty() {
            None
        } else {
            Some(agent.system_prompt.clone())
        }
    });
    if let Some(system_prompt) = system_prompt
        && !system_prompt.is_empty()
    {
        env.insert("KERNEL_SYSTEM_PROMPT".to_owned(), system_prompt);
    }

    if !missing.is_empty() {
        return Err(resolve_error_to_api(ResolveError::Missing(missing)));
    }

    tracing::debug!(
        action = "session_env",
        agent_id = %agent.agent_id,
        harness = agent.harness.as_str(),
        env_var_count = env.len(),
        has_connection = agent.connection_id.is_some(),
        "session environment prepared"
    );
    Ok(env)
}

struct StreamingTurn {
    turn_id: String,
    session_id: String,
    agent_host_session_id: String,
    message: String,
    assistant_message_id: String,
    assistant_created_at: String,
    stream: Arc<Mutex<ActiveTurnStreamState>>,
    _active_turn: ActiveTurnGuard,
}

struct ActiveTurnGuard {
    state: AppState,
    session_id: String,
    turn_id: String,
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        match self.state.active_turns.lock() {
            Ok(mut active_turns) => {
                if active_turns
                    .get(&self.session_id)
                    .is_some_and(|turn| turn.turn_id == self.turn_id)
                {
                    active_turns.remove(&self.session_id);
                    tracing::debug!(
                        action = "active_turn_guard_drop",
                        session_id = %self.session_id,
                        turn_id = %self.turn_id,
                        active_turn_count = active_turns.len(),
                        "active turn cleared"
                    );
                }
            }
            Err(_error) => {
                tracing::error!(
                    session_id = %self.session_id,
                    turn_id = %self.turn_id,
                    "active turn lock poisoned while clearing turn"
                );
            }
        }
    }
}

type NdjsonReceiver = mpsc::Receiver<StreamItem>;
const STREAM_SUBSCRIBER_CHANNEL_CAPACITY: usize = 256;

fn start_streaming_turn(
    state: &AppState,
    session_id: &str,
    message: String,
) -> Result<(StreamingTurn, NdjsonReceiver), ApiError> {
    let mut session = require_chat_session(state, session_id)?;
    let turn_id = Uuid::now_v7().simple().to_string();
    let user_message = MessageRecord::new(
        Uuid::now_v7().simple().to_string(),
        session.session_id.clone(),
        MessageRole::User,
        message.clone(),
    );
    let assistant_message_id = Uuid::now_v7().simple().to_string();
    let assistant_created_at = utc_now();
    let mut assistant_message = MessageRecord::new(
        assistant_message_id.clone(),
        session.session_id.clone(),
        MessageRole::Assistant,
        "",
    );
    assistant_created_at.clone_into(&mut assistant_message.created_at);
    let stream = Arc::new(Mutex::new(ActiveTurnStreamState {
        subscribers: Vec::new(),
        final_payload: None,
    }));
    let active_turn = begin_active_turn(
        state,
        &session.session_id,
        ActiveTurnRecord {
            turn_id: turn_id.clone(),
            user_message_id: user_message.message_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
            stream: Some(stream.clone()),
        },
    )?;
    let receiver = subscribe_stream_state(&stream)?;

    "busy".clone_into(&mut session.status);
    session.updated_at = utc_now();
    state.sessions.update(session.clone())?;
    state.sessions.append_message(&user_message)?;
    state.sessions.append_message(&assistant_message)?;

    tracing::info!(
        action = "start_streaming_turn",
        session_id = %session.session_id,
        turn_id = %turn_id,
        kernel_session_id = %session.agent_host_session_id,
        message_char_count = message.chars().count(),
        "streaming turn initialized"
    );

    Ok((
        StreamingTurn {
            turn_id,
            session_id: session.session_id,
            agent_host_session_id: session.agent_host_session_id,
            message,
            assistant_message_id,
            assistant_created_at,
            stream,
            _active_turn: active_turn,
        },
        receiver,
    ))
}

fn begin_active_turn(
    state: &AppState,
    session_id: &str,
    turn: ActiveTurnRecord,
) -> Result<ActiveTurnGuard, ApiError> {
    let mut active_turns = state
        .active_turns
        .lock()
        .map_err(|_error| ApiError::internal("active turn lock poisoned".to_owned()))?;
    if let Some(existing_turn) = active_turns.get(session_id) {
        tracing::warn!(
            action = "begin_active_turn",
            session_id = %session_id,
            turn_id = %turn.turn_id,
            existing_turn_id = %existing_turn.turn_id,
            error_kind = "active_turn_conflict",
            "session already has active turn"
        );
        return Err(ApiError::conflict(format!(
            "session {session_id:?} already has active turn {:?}",
            existing_turn.turn_id.as_str()
        )));
    }
    let turn_id = turn.turn_id.clone();
    active_turns.insert(session_id.to_owned(), turn);
    let active_turn_count = active_turns.len();
    drop(active_turns);
    tracing::info!(
        action = "begin_active_turn",
        session_id = %session_id,
        turn_id = %turn_id,
        active_turn_count,
        "active turn registered"
    );
    Ok(ActiveTurnGuard {
        state: state.clone(),
        session_id: session_id.to_owned(),
        turn_id,
    })
}

#[allow(clippy::too_many_lines)]
async fn run_streaming_turn(state: AppState, turn: StreamingTurn) {
    let mut events = Vec::new();
    let mut completed = false;
    let mut error = None;

    tracing::info!(
        action = "run_streaming_turn",
        session_id = %turn.session_id,
        turn_id = %turn.turn_id,
        kernel_session_id = %turn.agent_host_session_id,
        "upstream stream starting"
    );

    match state
        .agent_host
        .stream_message(&turn.agent_host_session_id, &turn.message)
        .await
    {
        Ok(mut stream) => {
            tracing::info!(
                action = "run_streaming_turn",
                session_id = %turn.session_id,
                turn_id = %turn.turn_id,
                kernel_session_id = %turn.agent_host_session_id,
                "upstream stream opened"
            );
            loop {
                match stream.next_event().await {
                    Ok(Some(event)) => {
                        let event_type = kernel_event_type(&event).to_owned();
                        let update_type = kernel_event_update_type(&event).map(ToOwned::to_owned);
                        events.push(event);
                        let assistant_message = assistant_message_from_events(
                            &turn.session_id,
                            &turn.assistant_message_id,
                            &turn.assistant_created_at,
                            &events,
                        );
                        if let Err(store_error) = state.sessions.update_message(&assistant_message)
                        {
                            error = Some(store_error.to_string());
                            tracing::error!(
                                action = "run_streaming_turn",
                                session_id = %turn.session_id,
                                turn_id = %turn.turn_id,
                                kernel_session_id = %turn.agent_host_session_id,
                                event_count = events.len(),
                                error_kind = "store_update_message",
                                "streaming turn failed to persist event"
                            );
                            break;
                        }

                        tracing::debug!(
                            action = "run_streaming_turn",
                            session_id = %turn.session_id,
                            turn_id = %turn.turn_id,
                            kernel_session_id = %turn.agent_host_session_id,
                            event_count = events.len(),
                            event_type = %event_type,
                            update_type = ?update_type,
                            "streaming event processed"
                        );

                        let event = Value::Object(events.last().cloned().unwrap_or_default());
                        let sent = send_stream_item(
                            &turn.stream,
                            &json!({
                                "type": "event",
                                "event": event,
                            }),
                            false,
                        );
                        if !sent {
                            tracing::warn!(
                                action = "run_streaming_turn",
                                session_id = %turn.session_id,
                                turn_id = %turn.turn_id,
                                kernel_session_id = %turn.agent_host_session_id,
                                event_count = events.len(),
                                error_kind = "stream_receiver_closed",
                                "streaming event send failed"
                            );
                        }
                    }
                    Ok(None) => {
                        completed = true;
                        tracing::info!(
                            action = "run_streaming_turn",
                            session_id = %turn.session_id,
                            turn_id = %turn.turn_id,
                            kernel_session_id = %turn.agent_host_session_id,
                            event_count = events.len(),
                            "upstream stream completed"
                        );
                        break;
                    }
                    Err(stream_error) => {
                        error = Some(stream_error.to_string());
                        tracing::warn!(
                            action = "run_streaming_turn",
                            session_id = %turn.session_id,
                            turn_id = %turn.turn_id,
                            kernel_session_id = %turn.agent_host_session_id,
                            event_count = events.len(),
                            error_kind = "upstream_stream_event",
                            "upstream stream event failed"
                        );
                        break;
                    }
                }
            }
        }
        Err(stream_error) => {
            error = Some(stream_error.to_string());
            tracing::warn!(
                action = "run_streaming_turn",
                session_id = %turn.session_id,
                turn_id = %turn.turn_id,
                kernel_session_id = %turn.agent_host_session_id,
                error_kind = "upstream_stream_start",
                "upstream stream failed to start"
            );
        }
    }

    let final_payload =
        match finalize_streaming_turn(&state, &turn, &events, completed, error.as_deref()).await {
            Ok(payload) => payload,
            Err(finalize_error) => {
                tracing::error!(
                    action = "run_streaming_turn",
                    session_id = %turn.session_id,
                    turn_id = %turn.turn_id,
                    kernel_session_id = %turn.agent_host_session_id,
                    error_kind = finalize_error.error_kind(),
                    status = finalize_error.status.as_u16(),
                    "streaming turn finalization failed"
                );
                json!({
                    "type": "final",
                    "turn_id": turn.turn_id.clone(),
                    "completed": false,
                    "error": finalize_error.detail,
                })
            }
        };
    let sent = send_stream_item(&turn.stream, &final_payload, true);
    tracing::info!(
        action = "run_streaming_turn",
        session_id = %turn.session_id,
        turn_id = %turn.turn_id,
        kernel_session_id = %turn.agent_host_session_id,
        completed,
        has_error = error.is_some(),
        event_count = events.len(),
        final_sent = sent,
        "streaming turn finished"
    );
}

async fn finalize_streaming_turn(
    state: &AppState,
    turn: &StreamingTurn,
    events: &[KernelEvent],
    completed: bool,
    error: Option<&str>,
) -> Result<Value, ApiError> {
    let assistant_message = assistant_message_from_events(
        &turn.session_id,
        &turn.assistant_message_id,
        &turn.assistant_created_at,
        events,
    );
    state.sessions.update_message(&assistant_message.clone())?;

    let mut session = require_session(state, &turn.session_id)?;
    if let Ok(upstream) = state
        .agent_host
        .get_session(&turn.agent_host_session_id)
        .await
    {
        if let Ok(status) = string_field(&upstream, "status") {
            session.status = status;
        }
    } else if error.is_some() {
        "error".clone_into(&mut session.status);
    }
    session.updated_at = utc_now();
    state.sessions.update(session.clone())?;

    tracing::info!(
        action = "finalize_streaming_turn",
        session_id = %turn.session_id,
        turn_id = %turn.turn_id,
        kernel_session_id = %turn.agent_host_session_id,
        status = %session.status,
        completed,
        has_error = error.is_some(),
        event_count = events.len(),
        tool_call_count = assistant_message.tool_calls.len(),
        "streaming turn finalized"
    );

    let mut payload = json!({
        "type": "final",
        "session": session.summary(),
        "assistant_message": assistant_message.summary(),
        "events": events,
        "turn_id": turn.turn_id,
        "completed": completed,
    });
    if let Some(error) = error
        && let Value::Object(object) = &mut payload
    {
        object.insert("error".to_owned(), json!(error));
    }
    Ok(payload)
}

fn subscribe_active_turn(
    state: &AppState,
    session_id: &str,
    turn_id: &str,
) -> Result<NdjsonReceiver, ApiError> {
    let active_turns = state
        .active_turns
        .lock()
        .map_err(|_error| ApiError::internal("active turn lock poisoned".to_owned()))?;
    let stream = {
        let Some(turn) = active_turns.get(session_id) else {
            return Err(ApiError::not_found(format!("turn not found: {turn_id}")));
        };
        if turn.turn_id != turn_id {
            return Err(ApiError::not_found(format!("turn not found: {turn_id}")));
        }
        turn.stream
            .clone()
            .ok_or_else(|| ApiError::not_found(format!("turn not found: {turn_id}")))?
    };
    drop(active_turns);
    subscribe_stream_state(&stream)
}

fn subscribe_stream_state(
    stream: &Arc<Mutex<ActiveTurnStreamState>>,
) -> Result<NdjsonReceiver, ApiError> {
    let (sender, receiver) = mpsc::channel(STREAM_SUBSCRIBER_CHANNEL_CAPACITY);
    let final_payload = {
        let mut stream = stream
            .lock()
            .map_err(|_error| ApiError::internal("active turn stream lock poisoned".to_owned()))?;
        if let Some(final_payload) = &stream.final_payload {
            Some(final_payload.clone())
        } else {
            stream.subscribers.push(sender.clone());
            None
        }
    };
    if let Some(final_payload) = final_payload {
        sender.try_send(Ok(final_payload)).map_err(|_error| {
            ApiError::internal("failed to enqueue final stream payload".to_owned())
        })?;
    }
    Ok(receiver)
}

fn send_stream_item(
    stream: &Arc<Mutex<ActiveTurnStreamState>>,
    value: &Value,
    close: bool,
) -> bool {
    let line = match ndjson_line_bytes(value) {
        Ok(line) => line,
        Err(_error) => return false,
    };
    let mut sent = false;
    let mut stream = match stream.lock() {
        Ok(stream) => stream,
        Err(_error) => return false,
    };

    if close {
        stream.final_payload = Some(line.clone());
    }

    stream.subscribers.retain(|subscriber| {
        let keep = match subscriber.try_send(Ok(line.clone())) {
            Ok(()) => {
                sent = true;
                true
            }
            Err(mpsc::error::TrySendError::Full(_line)) => false,
            Err(mpsc::error::TrySendError::Closed(_line)) => false,
        };
        keep && !close
    });

    if close {
        stream.subscribers.clear();
    }

    sent
}

async fn run_turn(state: &AppState, session_id: &str, message: &str) -> Result<Value, ApiError> {
    let mut session = require_chat_session(state, session_id)?;
    let turn_id = Uuid::now_v7().simple().to_string();
    let user_message = MessageRecord::new(
        Uuid::now_v7().simple().to_string(),
        session.session_id.clone(),
        MessageRole::User,
        message,
    );
    let assistant_message_id = Uuid::now_v7().simple().to_string();
    let assistant_created_at = utc_now();
    let _active_turn = begin_active_turn(
        state,
        &session.session_id,
        ActiveTurnRecord {
            turn_id: turn_id.clone(),
            user_message_id: user_message.message_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
            stream: None,
        },
    )?;
    tracing::info!(
        action = "run_turn",
        session_id = %session.session_id,
        turn_id = %turn_id,
        kernel_session_id = %session.agent_host_session_id,
        message_char_count = message.chars().count(),
        "synchronous turn started"
    );
    "busy".clone_into(&mut session.status);
    session.updated_at = utc_now();
    state.sessions.update(session.clone())?;
    state.sessions.append_message(&user_message)?;

    let events = state
        .agent_host
        .send_message(&session.agent_host_session_id, message)
        .await?;
    tracing::info!(
        action = "run_turn",
        session_id = %session.session_id,
        turn_id = %turn_id,
        kernel_session_id = %session.agent_host_session_id,
        event_count = events.len(),
        "synchronous turn upstream completed"
    );
    let assistant_message = assistant_message_from_events(
        &session.session_id,
        &assistant_message_id,
        &assistant_created_at,
        &events,
    );
    state.sessions.append_message(&assistant_message.clone())?;
    let mut session = require_session(state, session_id)?;
    if let Ok(upstream) = state
        .agent_host
        .get_session(&session.agent_host_session_id)
        .await
        && let Ok(status) = string_field(&upstream, "status")
    {
        session.status = status;
    }
    session.updated_at = utc_now();
    state.sessions.update(session.clone())?;
    tracing::info!(
        action = "run_turn",
        session_id = %session.session_id,
        turn_id = %turn_id,
        kernel_session_id = %session.agent_host_session_id,
        status = %session.status,
        event_count = events.len(),
        tool_call_count = assistant_message.tool_calls.len(),
        "synchronous turn completed"
    );
    Ok(json!({
        "session": session.summary(),
        "assistant_message": assistant_message.summary(),
        "events": events,
        "turn_id": turn_id,
        "completed": true,
    }))
}

fn kernel_event_type(event: &KernelEvent) -> &str {
    event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn kernel_event_update_type(event: &KernelEvent) -> Option<&str> {
    session_update(event).and_then(|update| update.get("sessionUpdate").and_then(Value::as_str))
}

fn assistant_message_from_events(
    session_id: &str,
    message_id: &str,
    created_at: &str,
    events: &[KernelEvent],
) -> MessageRecord {
    let mut message = MessageRecord::new(
        message_id.to_owned(),
        session_id.to_owned(),
        MessageRole::Assistant,
        flatten_text(events),
    );
    created_at.clone_into(&mut message.created_at);
    message.reasoning = flatten_reasoning(events);
    message.tool_calls = extract_tool_calls(events);
    message
}

fn extract_tool_calls(events: &[KernelEvent]) -> Vec<ToolCallRecord> {
    let mut calls = Vec::new();
    let mut by_id = BTreeMap::new();
    let mut content = String::new();

    for event in events {
        match event.get("type").and_then(Value::as_str) {
            Some("text_delta") => {
                if let Some(chunk) = event.get("content").and_then(Value::as_str) {
                    content.push_str(chunk);
                }
                continue;
            }
            Some("tool_call") => {
                if let Some(tool) = event.get("tool").and_then(Value::as_str) {
                    let mut call = ToolCallRecord::new(tool);
                    call.input = event.get("input").and_then(json_string);
                    call.content_offset = Some(trimmed_char_count(&content));
                    calls.push(call);
                }
                continue;
            }
            Some("tool_result") => {
                if let Some(tool) = event.get("tool").and_then(Value::as_str) {
                    apply_legacy_tool_result(&mut calls, tool, event.get("output"));
                }
                continue;
            }
            _ => {}
        }

        let update = session_update(event);
        match update.and_then(|update| update.get("sessionUpdate").and_then(Value::as_str)) {
            Some("agent_message_chunk") => {
                if let Some(update) = update {
                    content.push_str(&content_text(update.get("content")));
                }
            }
            Some("tool_call" | "tool_call_update") => {
                if let Some(update) = update {
                    upsert_tool_call(&mut calls, &mut by_id, update, trimmed_char_count(&content));
                }
            }
            _ => {}
        }
    }

    calls
}

fn apply_legacy_tool_result(calls: &mut [ToolCallRecord], tool: &str, output: Option<&Value>) {
    let Some(output) = output.and_then(Value::as_str) else {
        return;
    };
    if let Some(call) = calls
        .iter_mut()
        .find(|call| call.tool == tool && call.output.is_none())
    {
        call.output = Some(output.to_owned());
    }
}

fn session_update(event: &KernelEvent) -> Option<&JsonObject> {
    if event.get("type").and_then(Value::as_str) != Some("session/update") {
        return None;
    }
    event.get("update").and_then(Value::as_object)
}

fn upsert_tool_call(
    calls: &mut Vec<ToolCallRecord>,
    by_id: &mut BTreeMap<String, usize>,
    update: &JsonObject,
    content_offset: usize,
) {
    let tool_call_id = optional_string(update.get("toolCallId"));
    let mut index = tool_call_id
        .as_ref()
        .and_then(|tool_call_id| by_id.get(tool_call_id).copied());
    if index.is_none() {
        let title = optional_non_empty_string(update.get("title"))
            .or_else(|| {
                tool_call_id
                    .as_ref()
                    .filter(|value| !value.is_empty())
                    .cloned()
            })
            .unwrap_or_else(|| "tool".to_owned());
        let new_index = calls.len();
        index = Some(new_index);
        calls.push(ToolCallRecord {
            tool: title,
            tool_call_id: tool_call_id.clone(),
            status: None,
            kind: None,
            input: None,
            output: None,
            content_offset: Some(content_offset),
        });
        if let Some(tool_call_id) = tool_call_id {
            by_id.insert(tool_call_id, new_index);
        }
    }

    let Some(call) = index.and_then(|index| calls.get_mut(index)) else {
        return;
    };
    if let Some(title) = optional_non_empty_string(update.get("title")) {
        call.tool = title;
    }
    if let Some(status) = optional_non_empty_string(update.get("status")) {
        call.status = Some(status);
    }
    if let Some(kind) = optional_non_empty_string(update.get("kind")) {
        call.kind = Some(kind);
    }
    if let Some(raw_input) = update.get("rawInput") {
        call.input = json_string(raw_input);
    }
    if let Some(output) = tool_output(update) {
        call.output = Some(output);
    }
    if let Some(chunk) = terminal_output_delta(update) {
        call.output.get_or_insert_with(String::new).push_str(&chunk);
    }
}

/// Incremental terminal output from ACP agents that stream shell tool output
/// through `_meta` (for example the `pi-acp` adapter) instead of tool content.
fn terminal_output_delta(update: &JsonObject) -> Option<String> {
    let data = update
        .get("_meta")?
        .get("terminal_output")?
        .get("data")?
        .as_str()?;
    if data.is_empty() {
        None
    } else {
        Some(data.to_owned())
    }
}

fn tool_output(update: &JsonObject) -> Option<String> {
    if let Some(raw_output) = update.get("rawOutput") {
        return json_string(raw_output);
    }
    let content = content_text(update.get("content"));
    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}

fn json_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        _ => serde_json::to_string_pretty(value).ok(),
    }
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(ToOwned::to_owned)
}

fn optional_non_empty_string(value: Option<&Value>) -> Option<String> {
    optional_string(value).filter(|value| !value.is_empty())
}

fn trimmed_char_count(value: &str) -> usize {
    value.trim().chars().count()
}

fn flatten_text(events: &[KernelEvent]) -> String {
    let mut chunks = Vec::new();
    for event in events {
        if event.get("type").and_then(Value::as_str) == Some("text_delta") {
            if let Some(content) = event.get("content").and_then(Value::as_str) {
                chunks.push(content.to_owned());
            }
            continue;
        }
        if let Some(update) = event.get("update").and_then(Value::as_object)
            && update.get("sessionUpdate").and_then(Value::as_str) == Some("agent_message_chunk")
        {
            chunks.push(content_text(update.get("content")));
        }
    }
    chunks.join("").trim().to_owned()
}

fn flatten_reasoning(events: &[KernelEvent]) -> String {
    let mut chunks = Vec::new();
    for event in events {
        if event.get("type").and_then(Value::as_str) == Some("reasoning_delta") {
            if let Some(content) = event.get("content").and_then(Value::as_str) {
                chunks.push(content.to_owned());
            }
            continue;
        }
        if let Some(update) = event.get("update").and_then(Value::as_object)
            && update.get("sessionUpdate").and_then(Value::as_str) == Some("agent_thought_chunk")
        {
            chunks.push(content_text(update.get("content")));
        }
    }
    chunks.join("").trim().to_owned()
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items.iter().map(|item| content_text(Some(item))).collect(),
        Some(Value::Object(object)) => {
            if object.get("type").and_then(Value::as_str) == Some("text") {
                return object
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
            }
            if object.get("type").and_then(Value::as_str) == Some("content") {
                return content_text(object.get("content"));
            }
            // A terminal block is a handle to live output, not content: the
            // output itself arrives in later `_meta.terminal_output` updates.
            if object.get("type").and_then(Value::as_str) == Some("terminal") {
                return String::new();
            }
            serde_json::to_string(&Value::Object(object.clone())).unwrap_or_default()
        }
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn ndjson_line_bytes(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    Ok(line)
}

fn ndjson_stream_response(receiver: NdjsonReceiver) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-cache"),
            (HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(ReceiverStream::new(receiver)),
    )
        .into_response()
}

fn session_summary(state: &AppState, session: &SessionRecord) -> Result<Value, ApiError> {
    let mut summary = match session.summary() {
        Value::Object(summary) => summary,
        _ => JsonObject::new(),
    };
    if let Some(active_turn) = active_turn_summary(state, &session.session_id)? {
        summary.insert("active_turn".to_owned(), active_turn);
    }
    Ok(Value::Object(summary))
}

fn session_detail(state: &AppState, session: &SessionRecord) -> Result<Value, ApiError> {
    let mut detail = match session.detail() {
        Value::Object(detail) => detail,
        _ => JsonObject::new(),
    };
    if let Some(active_turn) = active_turn_summary(state, &session.session_id)? {
        detail.insert("active_turn".to_owned(), active_turn);
    }
    Ok(Value::Object(detail))
}

fn active_turn_summary(state: &AppState, session_id: &str) -> Result<Option<Value>, ApiError> {
    let active_turns = state
        .active_turns
        .lock()
        .map_err(|_error| ApiError::internal("active turn lock poisoned".to_owned()))?;
    Ok(active_turns.get(session_id).map(|turn| {
        json!({
            "turn_id": turn.turn_id.as_str(),
            "user_message_id": turn.user_message_id.as_str(),
            "assistant_message_id": turn.assistant_message_id.as_str(),
            "status": "running",
        })
    }))
}

fn session_workspace_mounts(
    agent_mounts: &[WorkspaceMountRecord],
    request_mounts: &[WorkspaceMountRecord],
) -> Vec<WorkspaceMountRecord> {
    let mut mounts = agent_mounts.to_vec();
    for request_mount in request_mounts {
        if let Some(existing) = mounts
            .iter_mut()
            .find(|mount| mount.workspace_id == request_mount.workspace_id)
        {
            *existing = request_mount.clone();
        } else {
            mounts.push(request_mount.clone());
        }
    }
    mounts
}

fn require_agent(state: &AppState, agent_id: &str) -> Result<AgentRecord, ApiError> {
    state
        .agents
        .get(agent_id)?
        .ok_or_else(|| ApiError::not_found(format!("agent {agent_id:?} not found")))
}

fn require_connection(state: &AppState, connection_id: &str) -> Result<ConnectionRecord, ApiError> {
    state
        .connections
        .get(connection_id)?
        .ok_or_else(|| ApiError::not_found(format!("connection {connection_id:?} not found")))
}

fn require_gateway(state: &AppState, gateway_id: &str) -> Result<GatewayRecord, ApiError> {
    state
        .gateways
        .get(gateway_id)?
        .ok_or_else(|| ApiError::not_found(format!("gateway {gateway_id:?} not found")))
}

fn require_workspace(state: &AppState, workspace_id: &str) -> Result<WorkspaceRecord, ApiError> {
    state
        .workspaces
        .get(workspace_id)?
        .ok_or_else(|| ApiError::not_found(format!("workspace {workspace_id:?} not found")))
}

fn require_ready_workspace(
    state: &AppState,
    workspace_id: &str,
) -> Result<WorkspaceRecord, ApiError> {
    let workspace = require_workspace(state, workspace_id)?;
    if workspace.status != WorkspaceStatus::Ready {
        return Err(ApiError::conflict(format!(
            "workspace {workspace_id:?} is not ready"
        )));
    }
    Ok(workspace)
}

fn require_session(state: &AppState, session_id: &str) -> Result<SessionRecord, ApiError> {
    state
        .sessions
        .get(session_id)?
        .ok_or_else(|| ApiError::not_found(format!("session {session_id:?} not found")))
}

fn require_chat_session(state: &AppState, session_id: &str) -> Result<SessionRecord, ApiError> {
    let session = require_session(state, session_id)?;
    if session.interaction_mode == InteractionMode::Cli {
        return Err(ApiError::conflict(format!(
            "session {session_id:?} uses CLI interaction mode"
        )));
    }
    Ok(session)
}

fn validate_workspace_mounts(
    state: &AppState,
    mounts: &[WorkspaceMountRecord],
) -> Result<(), ApiError> {
    let mut seen = BTreeSet::new();
    for mount in mounts {
        validate_workspace_id(&mount.workspace_id)?;
        if !seen.insert(mount.workspace_id.as_str()) {
            return Err(ApiError::unprocessable(format!(
                "workspace {:?} is mounted more than once",
                mount.workspace_id
            )));
        }
        require_ready_workspace(state, &mount.workspace_id)?;
    }
    Ok(())
}

fn workspace_in_use(state: &AppState, workspace_id: &str) -> Result<bool, ApiError> {
    Ok(state.agents.list()?.into_iter().any(|agent| {
        agent
            .workspace_mounts
            .iter()
            .any(|mount| mount.workspace_id == workspace_id)
    }))
}

fn parse_harness(raw: &str) -> Result<HarnessName, ApiError> {
    HarnessName::from_str(raw).map_err(ApiError::from)
}

fn parse_gateway_type(raw: &str) -> Result<GatewayType, ApiError> {
    GatewayType::from_str(raw).map_err(ApiError::from)
}

fn string_field(object: &JsonObject, field: &'static str) -> Result<String, ApiError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ApiError::bad_gateway(format!(
                "agent_host response missing string field {field:?}"
            ))
        })
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct UpdateKernelConfigRequest {
    #[serde(default)]
    env_vars: String,
}

#[derive(Debug, Deserialize)]
struct CreateConnectionRequest {
    connection_id: String,
    name: String,
    url: String,
    #[serde(default)]
    api_flavor: ConnectionApiFlavor,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateConnectionRequest {
    name: Option<String>,
    url: Option<String>,
    api_flavor: Option<ConnectionApiFlavor>,
    api_key: Option<String>,
    api_key_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateAgentRequest {
    agent_id: String,
    name: String,
    #[serde(default = "default_harness")]
    harness: HarnessName,
    #[serde(default = "default_agent_system_prompt")]
    system_prompt: String,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    env_vars: String,
    connection_id: Option<String>,
    #[serde(default)]
    cli: Option<AgentCliRequest>,
    #[serde(default)]
    workspace_mounts: Vec<WorkspaceMountRecord>,
}

#[derive(Debug, Deserialize)]
struct UpdateAgentRequest {
    name: Option<String>,
    harness: Option<HarnessName>,
    system_prompt: Option<String>,
    skills: Option<Vec<String>>,
    env_vars: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_string_field")]
    connection_id: NullableStringField,
    #[serde(default, deserialize_with = "deserialize_nullable_cli_field")]
    cli: NullableCliField,
    workspace_mounts: Option<Vec<WorkspaceMountRecord>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentCliRequest {
    harness: CliHarnessName,
    #[serde(default)]
    connection_id: Option<String>,
}

impl AgentCliRequest {
    fn into_record(self) -> AgentCliRecord {
        AgentCliRecord {
            harness: self.harness,
            connection_id: self.connection_id,
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateWorkspaceRequest {
    workspace_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct UpdateWorkspaceRequest {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CloneWorkspaceRequest {
    workspace_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct SaveSessionWorkspaceRequest {
    workspace_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    agent_id: String,
    channel_name: Option<String>,
    client_type: Option<ClientType>,
    #[serde(default)]
    interaction_mode: InteractionMode,
    #[serde(default)]
    workspace_mounts: Vec<WorkspaceMountRecord>,
}

#[derive(Debug, Deserialize)]
struct SendMessageRequest {
    message: String,
}

#[derive(Debug, Deserialize)]
struct ContainerLogsQuery {
    tail: Option<u64>,
    #[serde(default, rename = "all")]
    all: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CreateSkillRequest {
    skill_id: String,
    files: BTreeMap<String, String>,
    creator_agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateSkillRequest {
    files: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct CreateGatewayRequest {
    gateway_id: String,
    name: String,
    gateway_type: GatewayType,
    agent_id: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    env_vars: String,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct UpdateGatewayRequest {
    name: Option<String>,
    agent_id: Option<String>,
    enabled: Option<bool>,
    env_vars: Option<String>,
    secrets: Option<BTreeMap<String, String>>,
}

const fn default_harness() -> HarnessName {
    HarnessName::Acp
}

fn default_agent_system_prompt() -> String {
    DEFAULT_AGENT_SYSTEM_PROMPT.to_owned()
}

#[derive(Debug, Default)]
enum NullableStringField {
    #[default]
    Missing,
    Null,
    Value(String),
}

fn deserialize_nullable_string_field<'de, D>(
    deserializer: D,
) -> Result<NullableStringField, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
        .map(|value| value.map_or(NullableStringField::Null, NullableStringField::Value))
}

#[derive(Debug, Default)]
enum NullableCliField {
    #[default]
    Missing,
    Null,
    Value(AgentCliRequest),
}

fn deserialize_nullable_cli_field<'de, D>(deserializer: D) -> Result<NullableCliField, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<AgentCliRequest>::deserialize(deserializer)
        .map(|value| value.map_or(NullableCliField::Null, NullableCliField::Value))
}

const CONFIG_YAML_CONTENT_TYPE: &str = "application/yaml";

#[derive(Debug, Deserialize)]
struct ExportQuery {
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateSecretRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SetSecretValueRequest {
    value: String,
}

fn yaml_response(filename: &str, bytes: Vec<u8>) -> Result<Response, ApiError> {
    let mut response = (StatusCode::OK, bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(CONFIG_YAML_CONTENT_TYPE),
    );
    let disposition = format!("attachment; filename=\"{filename}\"");
    let value = HeaderValue::from_str(&disposition)
        .map_err(|error| ApiError::internal(format!("invalid content-disposition: {error}")))?;
    headers.insert(header::CONTENT_DISPOSITION, value);
    Ok(response)
}

fn bundle_response(filename: &str, bytes: Vec<u8>) -> Result<Response, ApiError> {
    let mut response = (StatusCode::OK, bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    let disposition = format!("attachment; filename=\"{filename}\"");
    let value = HeaderValue::from_str(&disposition)
        .map_err(|error| ApiError::internal(format!("invalid content-disposition: {error}")))?;
    headers.insert(header::CONTENT_DISPOSITION, value);
    Ok(response)
}

async fn export_config(
    State(state): State<AppState>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    match query.mode.as_deref() {
        Some("canonical") => {
            let bytes = to_canonical_yaml(&state.config_state.active())?.into_bytes();
            yaml_response("agentspace-config.canonical.yaml", bytes)
        }
        None | Some("source") => {
            let (bytes, kind) = state.config_state.source_export()?;
            match kind {
                SourceKind::Bundle => bundle_response("agentspace-config.zip", bytes),
                SourceKind::Yaml => yaml_response("agentspace-config.yaml", bytes),
            }
        }
        Some(other) => Err(ApiError::unprocessable(format!(
            "unsupported export mode {other:?}; expected \"source\" or \"canonical\""
        ))),
    }
}

fn spec_without<T: Serialize>(value: &T, drop_key: &str) -> Result<serde_yaml_ng::Value, ApiError> {
    let mut value = serde_yaml_ng::to_value(value)
        .map_err(|error| ApiError::internal(format!("failed to serialize resource: {error}")))?;
    if let serde_yaml_ng::Value::Mapping(map) = &mut value {
        map.remove(serde_yaml_ng::Value::String(drop_key.to_owned()));
    }
    Ok(value)
}

fn standalone_manifest(
    kind: &str,
    name: &str,
    spec: serde_yaml_ng::Value,
) -> Result<Vec<u8>, ApiError> {
    use serde_yaml_ng::Value;
    let mut metadata = serde_yaml_ng::Mapping::new();
    metadata.insert(
        Value::String("name".to_owned()),
        Value::String(name.to_owned()),
    );
    let mut root = serde_yaml_ng::Mapping::new();
    root.insert(
        Value::String("apiVersion".to_owned()),
        Value::String(config::document::API_VERSION.to_owned()),
    );
    root.insert(
        Value::String("kind".to_owned()),
        Value::String(kind.to_owned()),
    );
    root.insert(
        Value::String("metadata".to_owned()),
        Value::Mapping(metadata),
    );
    root.insert(Value::String("spec".to_owned()), spec);
    let text = serde_yaml_ng::to_string(&Value::Mapping(root))
        .map_err(|error| ApiError::internal(format!("failed to serialize manifest: {error}")))?;
    Ok(text.into_bytes())
}

async fn export_config_resource(
    State(state): State<AppState>,
    Path((kind, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let document = state.config_state.active();
    let missing = || {
        ApiError::not_found(format!(
            "{kind}/{name} was not found in the config document"
        ))
    };
    let (manifest_kind, spec) = match kind.as_str() {
        "secret" => {
            let item = document
                .spec
                .secrets
                .iter()
                .find(|item| item.name.as_str() == name)
                .ok_or_else(missing)?;
            ("SecretDeclaration", spec_without(item, "name")?)
        }
        "kernel-config" => {
            let item = document
                .spec
                .kernel_configs
                .iter()
                .find(|item| item.harness.as_str() == name)
                .ok_or_else(missing)?;
            ("KernelConfig", spec_without(item, "harness")?)
        }
        "connection" => {
            let item = document.connection(&name).ok_or_else(missing)?;
            ("Connection", spec_without(item, "id")?)
        }
        "skill" => {
            let item = document
                .spec
                .skills
                .iter()
                .find(|item| item.id == name)
                .ok_or_else(missing)?;
            ("Skill", spec_without(item, "id")?)
        }
        "agent" => {
            let item = document
                .spec
                .agents
                .iter()
                .find(|item| item.id == name)
                .ok_or_else(missing)?;
            ("Agent", spec_without(item, "id")?)
        }
        "gateway" => {
            let item = document.gateway(&name).ok_or_else(missing)?;
            ("Gateway", spec_without(item, "id")?)
        }
        other => {
            return Err(ApiError::not_found(format!(
                "unknown resource kind {other:?}"
            )));
        }
    };
    let bytes = standalone_manifest(manifest_kind, &name, spec)?;
    yaml_response(&format!("{kind}-{name}.yaml"), bytes)
}

async fn validate_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let kind = source_kind_from_headers(&headers);
    let builtins = builtin_skill_ids(&state).await;
    let unset = state.config_state.validate_source(&body, kind, &builtins)?;
    tracing::info!(
        route = "/config/validate",
        action = "validate_config",
        source_kind = kind.as_str(),
        "api handler completed"
    );
    Ok(Json(json!({ "valid": true, "unset_secrets": unset })))
}

async fn plan_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let kind = source_kind_from_headers(&headers);
    let builtins = builtin_skill_ids(&state).await;
    let (plan, unset) = state.config_state.plan_source(&body, kind, &builtins)?;
    tracing::info!(
        route = "/config/plan",
        action = "plan_config",
        source_kind = kind.as_str(),
        "api handler completed"
    );
    Ok(Json(json!({
        "plan": plan.to_json(),
        "unset_secrets": unset,
        // Callers may pass this back as `If-Match` to `/config/apply` for an
        // optimistic-concurrency check.
        "active_generation": state.config_state.active_generation(),
    })))
}

async fn apply_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let kind = source_kind_from_headers(&headers);
    let expected_generation = parse_if_match_generation(&headers)?;
    let builtins = builtin_skill_ids(&state).await;

    // Serialize the whole apply (prepare → stage skills → commit → reconcile
    // gateways) so two applies cannot interleave reconciliation.
    let _apply_guard = state.apply_lock.lock().await;

    let prepared = state
        .config_state
        .prepare(&body, kind, &builtins, expected_generation)?;

    // Stage every changed user skill in agent_host BEFORE the snapshot is
    // activated, so a fully-successful apply is never reported while agent_host
    // is stale. If staging fails, the snapshot is never committed and staged
    // skills are compensated back to their prior agent_host state.
    let mut reconciliation = Reconciliation::default();
    let staged = match stage_skills(&state, &prepared, &mut reconciliation).await {
        Ok(staged) => staged,
        Err(error) => return Err(error),
    };

    let outcome = match state.config_state.commit(prepared.clone()) {
        Ok(outcome) => outcome,
        Err(error) => {
            // Commit failed after skills were staged; restore agent_host to the
            // state it had before staging so it does not drift from the still
            // active document.
            compensate_skills(&state, &staged).await;
            return Err(error.into());
        }
    };

    reconcile_gateways(&state, &outcome, &mut reconciliation).await;

    tracing::info!(
        route = "/config/apply",
        action = "apply_config",
        generation = outcome.snapshot.generation,
        change_count = outcome.plan.entries.len(),
        source_kind = kind.as_str(),
        reconcile_failures = reconciliation.failures.len(),
        "configuration applied"
    );
    Ok(Json(json!({
        "generation": outcome.snapshot.generation,
        "source_sha256": outcome.snapshot.source_sha256,
        "semantic_sha256": outcome.snapshot.semantic_sha256,
        "plan": outcome.plan.to_json(),
        "unset_secrets": outcome.unset_secrets,
        "reconciliation": reconciliation.to_json(),
    })))
}

/// Parse an optional `If-Match` header carrying the expected active generation
/// for an optimistic-concurrency apply. Accepts a bare integer or a quoted `ETag`.
fn parse_if_match_generation(headers: &HeaderMap) -> Result<Option<i64>, ApiError> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    let raw = value
        .to_str()
        .map_err(|_| ApiError::unprocessable("If-Match header is not valid text".to_owned()))?
        .trim()
        .trim_matches('"');
    if raw.is_empty() || raw == "*" {
        return Ok(None);
    }
    let generation = raw.parse::<i64>().map_err(|_| {
        ApiError::unprocessable(format!(
            "If-Match must be the expected integer generation, got {raw:?}"
        ))
    })?;
    Ok(Some(generation))
}

/// Determine the source kind of a `/config/apply` body from its content type.
fn source_kind_from_headers(headers: &HeaderMap) -> SourceKind {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if content_type.contains("zip") || content_type.contains("octet-stream") {
        SourceKind::Bundle
    } else {
        SourceKind::Yaml
    }
}

/// Extract the inline file map from an `agent_host` skill JSON object.
fn skill_files_from_object(object: &JsonObject) -> BTreeMap<String, String> {
    object
        .get("files")
        .and_then(Value::as_object)
        .map(|files| {
            files
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|text| (name.clone(), text.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build a deterministic ZIP archive from an authored skill's inline files so a
/// user skill can be downloaded straight from the `ConfigDocument`.
fn zip_skill_files(files: &BTreeMap<String, String>) -> Result<Vec<u8>, ApiError> {
    use std::io::Write as _;

    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(
                zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
                    .unwrap_or_else(|_| zip::DateTime::default()),
            );
        for (name, contents) in files {
            writer.start_file(name, options).map_err(|error| {
                ApiError::internal(format!("could not encode skill zip: {error}"))
            })?;
            writer.write_all(contents.as_bytes()).map_err(|error| {
                ApiError::internal(format!("could not write skill zip: {error}"))
            })?;
        }
        writer.finish().map_err(|error| {
            ApiError::internal(format!("could not finalize skill zip: {error}"))
        })?;
    }
    Ok(buffer)
}

/// Validate that every skill referenced by an agent resolves to either a user
/// skill declared in the document or an installation-owned builtin.
///
/// The builtin set is sourced from `agent_host`. When `agent_host` is
/// unreachable the builtin set is unknowable, so a reference that is not in the
/// document is accepted here and deferred to the authoritative apply/validate
/// gate rather than blocking an interactive edit on a transient outage.
async fn validate_agent_skill_refs(state: &AppState, skills: &[String]) -> Result<(), ApiError> {
    if skills.is_empty() {
        return Ok(());
    }
    let builtins = match state.agent_host.list_skills().await {
        Ok(entries) => entries
            .into_iter()
            .filter(|entry| {
                entry
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| source == "builtin")
            })
            .filter_map(|entry| {
                entry
                    .get("skill_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<BTreeSet<String>>(),
        Err(error) => {
            tracing::warn!(
                action = "validate_agent_skill_refs",
                error_kind = "agent_host_error",
                error = %error,
                "could not list builtin skills; deferring skill-reference validation to apply"
            );
            return Ok(());
        }
    };
    for skill_id in skills {
        let in_document = config::adapter::skill_exists(&state.config_state, skill_id)?;
        if !in_document && !builtins.contains(skill_id) {
            return Err(ApiError::unprocessable(format!(
                "agent references unknown skill {skill_id:?}; it is neither a declared user skill \
                 nor an installation-owned builtin"
            )));
        }
    }
    Ok(())
}

/// Return the id of an authored agent that references `skill_id`, if any.
fn agent_referencing_skill(state: &AppState, skill_id: &str) -> Option<String> {
    let document = state.config_state.active();
    document
        .spec
        .agents
        .iter()
        .find(|agent| agent.skills.iter().any(|skill| skill == skill_id))
        .map(|agent| agent.id.clone())
}

/// Collect installation-owned builtin skill IDs from `agent_host` so agents may
/// reference them without declaring them in the config document. Typos in
/// non-builtin skill references still fail validation.
///
/// If `agent_host` is unreachable the builtin set cannot be determined; a
/// warning is logged and an empty set is returned, which conservatively rejects
/// unknown skill references rather than accepting them blindly.
async fn builtin_skill_ids(state: &AppState) -> BTreeSet<String> {
    match state.agent_host.list_skills().await {
        Ok(skills) => skills
            .into_iter()
            .filter(|skill| {
                skill
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| source == "builtin")
            })
            .filter_map(|skill| {
                skill
                    .get("skill_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect(),
        Err(error) => {
            tracing::warn!(
                action = "builtin_skill_ids",
                error_kind = "agent_host_error",
                error = %error,
                "could not list builtin skills; treating builtin set as empty"
            );
            BTreeSet::new()
        }
    }
}

/// Result of reconciling `agent_host` after a full-document apply.
#[derive(Default)]
struct Reconciliation {
    skills: Vec<String>,
    gateways: Vec<String>,
    failures: Vec<ReconcileFailure>,
}

struct ReconcileFailure {
    resource: String,
    detail: String,
}

impl Reconciliation {
    fn fail(&mut self, resource: impl Into<String>, detail: impl Into<String>) {
        self.failures.push(ReconcileFailure {
            resource: resource.into(),
            detail: detail.into(),
        });
    }

    fn to_json(&self) -> Value {
        json!({
            "ok": self.failures.is_empty(),
            "reconciled_skills": self.skills,
            "reconciled_gateways": self.gateways,
            "failures": self
                .failures
                .iter()
                .map(|failure| json!({
                    "resource": failure.resource,
                    "detail": failure.detail,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

/// Stage every changed user skill in `agent_host` from the prepared replacement
/// document, using the plan so unchanged skills are never rewritten. On any
/// failure the already-staged operations are compensated back to the previously
/// active skill set and an error is returned so the apply aborts before the
/// snapshot is committed.
/// A skill mutation staged in `agent_host`, recorded so it can be reverted if a
/// later step of the apply fails.
enum StagedSkillOp {
    Created(String),
    Updated(String, BTreeMap<String, String>),
    Deleted(String, BTreeMap<String, String>),
}

/// Reconcile `agent_host` user skills to the prepared replacement document
/// BEFORE the snapshot is activated. Every user skill in the document is
/// created or updated (unchanged skills are compared and skipped so they are
/// never rewritten), and user skills present in `agent_host` but absent from the
/// document are removed. On any failure the already-staged operations are
/// compensated back to their prior state and an error is returned so the apply
/// aborts before the snapshot is committed. Builtin skills are never touched.
async fn stage_skills(
    state: &AppState,
    prepared: &config::state::PreparedApply,
    reconciliation: &mut Reconciliation,
) -> Result<Vec<StagedSkillOp>, ApiError> {
    let host_user = match agent_host_user_skills(state).await {
        Ok(map) => map,
        Err(error) => {
            // The upstream user-skill state is unknown. We cannot safely compute
            // orphan removals (or even confirm nothing needs staging) without a
            // listing, so we must NOT commit a "successful" apply. Fail even when
            // the desired list is empty rather than silently proceeding and
            // leaving agent_host in an unknown/stale state.
            return Err(error);
        }
    };
    let mut performed: Vec<StagedSkillOp> = Vec::new();

    for skill in &prepared.document.spec.skills {
        let result = match host_user.get(&skill.id) {
            None => state
                .agent_host
                .create_skill(&skill.id, &skill.files)
                .await
                .map(|_| Some(StagedSkillOp::Created(skill.id.clone()))),
            Some(existing) if existing != &skill.files => state
                .agent_host
                .update_skill(&skill.id, &skill.files)
                .await
                .map(|_| Some(StagedSkillOp::Updated(skill.id.clone(), existing.clone()))),
            Some(_) => Ok(None),
        };
        match result {
            Ok(Some(op)) => {
                reconciliation.skills.push(skill.id.clone());
                performed.push(op);
            }
            Ok(None) => {}
            Err(error) => {
                compensate_skills(state, &performed).await;
                return Err(ApiError::bad_gateway(format!(
                    "failed to materialize skill {:?} in agent_host; apply aborted and staged \
                     skills were rolled back: {error}",
                    skill.id
                )));
            }
        }
    }

    let desired: BTreeSet<&str> = prepared
        .document
        .spec
        .skills
        .iter()
        .map(|skill| skill.id.as_str())
        .collect();
    for (id, files) in &host_user {
        if desired.contains(id.as_str()) {
            continue;
        }
        if let Err(error) = state.agent_host.delete_skill(id).await {
            compensate_skills(state, &performed).await;
            return Err(ApiError::bad_gateway(format!(
                "failed to remove stale skill {id:?} from agent_host; apply aborted and staged \
                 skills were rolled back: {error}"
            )));
        }
        reconciliation.skills.push(id.clone());
        performed.push(StagedSkillOp::Deleted(id.clone(), files.clone()));
    }

    Ok(performed)
}

/// Collect the current user-sourced skills from `agent_host` as an id → files
/// map. Builtin skills are excluded because they are installation-owned.
///
/// The list endpoint returns only summaries (which in production omit file
/// contents), so the full files for every user skill are fetched with
/// `GET /skills/{id}` before comparison/staging/compensation. A summary that
/// lacks files is never treated as an empty skill, which would otherwise cause
/// spurious rewrites or destructive comparisons.
async fn agent_host_user_skills(
    state: &AppState,
) -> Result<BTreeMap<String, BTreeMap<String, String>>, ApiError> {
    let listed = state.agent_host.list_skills().await.map_err(|error| {
        ApiError::bad_gateway(format!(
            "could not list agent_host skills to reconcile the apply: {error}"
        ))
    })?;
    let mut user_ids = Vec::new();
    for skill in listed {
        let is_builtin = skill
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|source| source == "builtin");
        if is_builtin {
            continue;
        }
        if let Some(id) = skill.get("skill_id").and_then(Value::as_str) {
            user_ids.push(id.to_owned());
        }
    }
    let mut user = BTreeMap::new();
    for id in user_ids {
        let detail = state.agent_host.get_skill(&id).await.map_err(|error| {
            ApiError::bad_gateway(format!(
                "could not fetch skill {id:?} details from agent_host to reconcile the apply: \
                 {error}"
            ))
        })?;
        let files = detail.get("files").and_then(Value::as_object).ok_or_else(|| {
            ApiError::bad_gateway(format!(
                "agent_host skill {id:?} details did not include a files map; refusing to treat \
                 it as empty"
            ))
        })?;
        let files = files
            .iter()
            .filter_map(|(name, value)| value.as_str().map(|text| (name.clone(), text.to_owned())))
            .collect::<BTreeMap<String, String>>();
        user.insert(id, files);
    }
    Ok(user)
}

/// Best-effort revert of previously staged skill operations after a staging or
/// commit failure, restoring `agent_host` to the state it had before staging.
async fn compensate_skills(state: &AppState, performed: &[StagedSkillOp]) {
    for op in performed.iter().rev() {
        let restore = match op {
            StagedSkillOp::Created(id) => state.agent_host.delete_skill(id).await,
            StagedSkillOp::Updated(id, files) => {
                state.agent_host.update_skill(id, files).await.map(|_| ())
            }
            StagedSkillOp::Deleted(id, files) => {
                state.agent_host.create_skill(id, files).await.map(|_| ())
            }
        };
        if let Err(error) = restore {
            let id = match op {
                StagedSkillOp::Created(id)
                | StagedSkillOp::Updated(id, _)
                | StagedSkillOp::Deleted(id, _) => id,
            };
            tracing::warn!(
                action = "compensate_skills",
                skill_id = %id,
                error = %error,
                "failed to compensate a staged skill after an aborted apply"
            );
        }
    }
}
/// Reconcile gateway containers to the applied document. Start/stop transitions
/// are driven by the plan, so a no-op or unrelated apply never restarts an
/// unchanged gateway. Any `agent_host` gateway absent from the applied document
/// is destroyed (orphan removal), which also covers plan deletions. Per-resource
/// failures are reported (not silently swallowed).
async fn reconcile_gateways(
    state: &AppState,
    outcome: &config::state::ApplyOutcome,
    result: &mut Reconciliation,
) {
    let document = outcome.document.as_ref();
    let upstream_gateways = match state.agent_host.list_gateways().await {
        Ok(list) => list
            .iter()
            .filter_map(|gateway| gateway.get("gateway_id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<String>>(),
        Err(error) => {
            result.fail("gateways", error.to_string());
            BTreeSet::new()
        }
    };

    // Destroy every upstream gateway that is no longer present in the applied
    // document (covers both plan deletions and orphans left in agent_host).
    let desired: BTreeSet<&str> = document
        .spec
        .gateways
        .iter()
        .map(|gateway| gateway.id.as_str())
        .collect();
    for id in &upstream_gateways {
        if desired.contains(id.as_str()) {
            continue;
        }
        if let Err(error) = state.agent_host.destroy_gateway(id).await {
            result.fail(format!("gateway/{id}"), error.to_string());
        } else {
            result.gateways.push(id.clone());
        }
    }

    for entry in &outcome.plan.entries {
        if entry.kind != "gateway" {
            continue;
        }
        match entry.action {
            config::plan::PlanAction::Create | config::plan::PlanAction::Update => {
                let Some(gateway) = document.gateway(&entry.id) else {
                    continue;
                };
                if gateway.enabled {
                    match start_gateway_by_id(
                        state,
                        &gateway.id,
                        GatewayStartFailureMode::Propagate,
                    )
                    .await
                    {
                        Ok(_) => result.gateways.push(gateway.id.clone()),
                        Err(error) => {
                            result.fail(format!("gateway/{}", gateway.id), error.detail.clone());
                        }
                    }
                } else if upstream_gateways.contains(&gateway.id) {
                    match stop_gateway_by_id(state, &gateway.id).await {
                        Ok(_) => result.gateways.push(gateway.id.clone()),
                        Err(error) => {
                            result.fail(format!("gateway/{}", gateway.id), error.detail.clone());
                        }
                    }
                }
            }
            config::plan::PlanAction::Delete | config::plan::PlanAction::NoOp => {}
        }
    }
}

async fn list_secrets(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let document = state.config_state.active();
    let secrets = state.config_state.secrets();
    let mut out = Vec::new();
    for declaration in &document.spec.secrets {
        let name = declaration.name.as_str();
        let is_set = secrets.is_set(name)?;
        let references = state.config_state.secret_reference_fields(name);
        let reference_count = references.len();
        out.push(json!({
            "name": name,
            "description": declaration.description,
            "is_set": is_set,
            "references": references,
            "reference_count": reference_count,
        }));
    }
    tracing::info!(
        route = "/secrets",
        action = "list_secrets",
        secret_count = out.len(),
        "api handler completed"
    );
    Ok(Json(Value::Array(out)))
}

async fn create_secret(
    State(state): State<AppState>,
    Json(payload): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let name = SecretName::new(payload.name.clone())
        .map_err(|error| ApiError::unprocessable(error.to_string()))?;
    let declaration = SecretDeclaration {
        name: name.clone(),
        description: payload.description.clone(),
    };
    let created = state.config_state.declare_secret(declaration)?;
    if !created {
        return Err(ApiError::conflict(format!(
            "secret {} is already declared",
            name.as_str()
        )));
    }
    tracing::info!(
        route = "/secrets",
        action = "create_secret",
        secret_name = name.as_str(),
        "api handler completed"
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "name": name.as_str(),
            "description": payload.description,
            "is_set": false,
            "references": Vec::<String>::new(),
            "reference_count": 0,
        })),
    ))
}

async fn delete_secret(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    match state.config_state.undeclare_secret(&name)? {
        config::state::SecretRemoval::Removed => {
            tracing::info!(
                route = "/secrets/:name",
                action = "delete_secret",
                secret_name = %name,
                "api handler completed"
            );
            Ok(StatusCode::NO_CONTENT)
        }
        config::state::SecretRemoval::NotDeclared => Err(ApiError::not_found(format!(
            "secret {name} is not declared"
        ))),
        config::state::SecretRemoval::ValueSet => Err(ApiError::conflict(format!(
            "secret {name} has a value set; clear the value before deleting the declaration"
        ))),
        config::state::SecretRemoval::Referenced(references) => Err(ApiError::conflict(format!(
            "secret {name} is referenced by {} field(s)",
            references.len()
        ))
        .with_extra(json!({ "references": references }))),
    }
}

async fn set_secret_value(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<SetSecretValueRequest>,
) -> Result<StatusCode, ApiError> {
    let declared = state.config_state.set_secret_value(&name, &payload.value)?;
    if !declared {
        return Err(ApiError::not_found(format!(
            "secret {name} must be declared before a value can be set"
        )));
    }
    tracing::info!(
        route = "/secrets/:name/value",
        action = "set_secret_value",
        secret_name = %name,
        "api handler completed"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_secret_value(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let removed = state.config_state.clear_secret_value(&name)?;
    tracing::info!(
        route = "/secrets/:name/value",
        action = "clear_secret_value",
        secret_name = %name,
        removed,
        "api handler completed"
    );
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!(
            "secret {name} has no value set"
        )))
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    detail: String,
    extra: Option<Value>,
}

impl ApiError {
    const fn not_found(detail: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            detail,
            extra: None,
        }
    }

    const fn conflict(detail: String) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            detail,
            extra: None,
        }
    }

    const fn unprocessable(detail: String) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail,
            extra: None,
        }
    }

    const fn bad_gateway(detail: String) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            detail,
            extra: None,
        }
    }

    const fn service_unavailable(detail: String) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            detail,
            extra: None,
        }
    }

    const fn gateway_timeout(detail: String) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            detail,
            extra: None,
        }
    }

    const fn payload_too_large(detail: String) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            detail,
            extra: None,
        }
    }

    const fn internal(detail: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail,
            extra: None,
        }
    }

    const fn not_implemented(detail: String) -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            detail,
            extra: None,
        }
    }

    #[must_use]
    fn with_extra(mut self, extra: Value) -> Self {
        self.extra = Some(extra);
        self
    }

    fn error_kind(&self) -> &'static str {
        match self.status {
            StatusCode::NOT_FOUND => "not_found",
            StatusCode::CONFLICT => "conflict",
            StatusCode::UNPROCESSABLE_ENTITY => "validation",
            StatusCode::BAD_GATEWAY => "bad_gateway",
            StatusCode::SERVICE_UNAVAILABLE => "service_unavailable",
            StatusCode::GATEWAY_TIMEOUT => "gateway_timeout",
            StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
            StatusCode::INTERNAL_SERVER_ERROR => "internal",
            status if status.is_client_error() => "client_error",
            status if status.is_server_error() => "server_error",
            _ => "error",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        let error_kind = self.error_kind();
        let detail = self.detail;
        if status.is_server_error() {
            tracing::error!(status = status.as_u16(), error_kind, "api error response");
        } else {
            tracing::warn!(status = status.as_u16(), error_kind, "api error response");
        }
        let mut body = json!({ "detail": detail });
        if let (Some(Value::Object(extra)), Some(map)) = (self.extra, body.as_object_mut()) {
            for (key, value) in extra {
                map.insert(key, value);
            }
        }
        (status, Json(body)).into_response()
    }
}

impl From<ValidationError> for ApiError {
    fn from(error: ValidationError) -> Self {
        Self::unprocessable(error.to_string())
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::AgentAlreadyExists { .. }
            | StoreError::ConnectionAlreadyExists { .. }
            | StoreError::GatewayAlreadyExists { .. }
            | StoreError::WorkspaceAlreadyExists { .. }
            | StoreError::SessionAlreadyExists { .. } => Self::conflict(error.to_string()),
            StoreError::AgentNotFound { .. }
            | StoreError::ConnectionNotFound { .. }
            | StoreError::GatewayNotFound { .. }
            | StoreError::WorkspaceNotFound { .. }
            | StoreError::SessionNotFound { .. } => Self::not_found(error.to_string()),
            StoreError::LockPoisoned { .. } | StoreError::Persistence { .. } => {
                Self::internal(error.to_string())
            }
        }
    }
}

impl From<SecretStoreError> for ApiError {
    fn from(error: SecretStoreError) -> Self {
        Self::internal(error.to_string())
    }
}

fn issue_json(issue: &ValidationIssue) -> Value {
    json!({
        "code": issue.code,
        "detail": issue.detail,
        "resource": issue.resource,
        "field": issue.field,
    })
}

impl From<ConfigError> for ApiError {
    fn from(error: ConfigError) -> Self {
        match error {
            ConfigError::Validation { issues } => {
                let detail = format!("configuration is invalid: {} issue(s)", issues.len());
                let rendered: Vec<Value> = issues.iter().map(issue_json).collect();
                Self::unprocessable(detail).with_extra(json!({ "issues": rendered }))
            }
            ConfigError::Parse { .. }
            | ConfigError::UnsupportedApiVersion { .. }
            | ConfigError::UnsupportedKind { .. }
            | ConfigError::Bundle { .. }
            | ConfigError::DuplicateResource { .. } => Self::unprocessable(error.to_string()),
            ConfigError::UnsupportedBundle => Self::not_implemented(error.to_string()),
            ConfigError::SecretDeclarationRemovalBlocked { ref names } => {
                Self::conflict(error.to_string())
                    .with_extra(json!({ "blocked_secrets": names.clone() }))
            }
            ConfigError::GenerationConflict { expected, actual } => {
                Self::conflict(error.to_string()).with_extra(json!({
                    "error": "generation_conflict",
                    "expected_generation": expected,
                    "active_generation": actual,
                }))
            }
            ConfigError::Serialize { .. } | ConfigError::CanonicalDrift => {
                Self::internal(error.to_string())
            }
        }
    }
}

fn resolve_error_to_api(error: ResolveError) -> ApiError {
    match error {
        ResolveError::Missing(missing) => {
            let names: BTreeSet<String> = missing.iter().map(|item| item.name.clone()).collect();
            let detail = format!(
                "cannot resolve configuration: {} secret value(s) are not set: {}",
                names.len(),
                names.iter().cloned().collect::<Vec<_>>().join(", ")
            );
            let rendered: Vec<Value> = missing
                .iter()
                .map(|item| json!({ "name": item.name, "field": item.field }))
                .collect();
            ApiError::conflict(detail).with_extra(json!({
                "error": "secret_values_unset",
                "missing_secrets": rendered,
            }))
        }
        ResolveError::Store(error) => ApiError::internal(error.to_string()),
    }
}

impl From<AgentHostError> for ApiError {
    fn from(error: AgentHostError) -> Self {
        match error {
            AgentHostError::HttpStatus { status, .. } if status == StatusCode::NOT_FOUND => {
                Self::not_found(format!("agent_host returned HTTP {status}"))
            }
            AgentHostError::HttpStatus { status, .. } if status.is_client_error() => Self {
                status,
                detail: format!("agent_host returned HTTP {status}"),
                extra: None,
            },
            other => Self::bad_gateway(other.to_string()),
        }
    }
}

impl From<MemoryProxyError> for ApiError {
    fn from(error: MemoryProxyError) -> Self {
        match error {
            MemoryProxyError::Timeout { .. } => Self::gateway_timeout(error.to_string()),
            MemoryProxyError::Unavailable { .. } => Self::service_unavailable(error.to_string()),
            MemoryProxyError::MalformedResponse { .. }
            | MemoryProxyError::ResponseTooLarge { .. }
            | MemoryProxyError::Http { .. }
            | MemoryProxyError::InvalidBaseUrl { .. }
            | MemoryProxyError::UrlCannotBeBase { .. } => Self::bad_gateway(error.to_string()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::too_many_lines)]
mod tests {
    use std::{
        collections::BTreeMap,
        convert::Infallible,
        error::Error,
        net::SocketAddr,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        extract::{Path, State},
        http::{HeaderValue, Method, Request, StatusCode, header},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::sleep};
    use tokio_stream::wrappers::ReceiverStream;
    use tower::ServiceExt;

    use super::send_stream_item;
    use crate::{
        ActiveTurnStreamState, AppConfig, AppState, agent_host::AgentHostClient, build_router,
    };

    #[test]
    fn extract_tool_calls_collects_streamed_terminal_output() {
        let events = [
            json!({
                "type": "session/update",
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "call_1",
                    "title": "cat notes.txt",
                    "kind": "execute",
                    "status": "pending",
                    "content": [{"type": "terminal", "terminalId": "call_1"}]
                }
            }),
            json!({
                "type": "session/update",
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "call_1",
                    "status": "in_progress",
                    "_meta": {"terminal_output": {"terminal_id": "call_1", "data": "hello "}}
                }
            }),
            json!({
                "type": "session/update",
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "call_1",
                    "status": "completed",
                    "_meta": {"terminal_output": {"terminal_id": "call_1", "data": "file\n"}}
                }
            }),
        ]
        .into_iter()
        .map(|event| match event {
            Value::Object(object) => object,
            other => panic!("event fixture should be an object: {other}"),
        })
        .collect::<Vec<_>>();

        let calls = super::extract_tool_calls(&events);

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "cat notes.txt");
        assert_eq!(calls[0].status.as_deref(), Some("completed"));
        assert_eq!(calls[0].output.as_deref(), Some("hello file\n"));
    }

    fn test_router() -> Result<Router, Box<dyn Error + Send + Sync>> {
        test_router_with_agent_host("http://127.0.0.1:9", Duration::from_millis(50))
    }

    fn test_router_with_agent_host(
        agent_host_base_url: &str,
        timeout: Duration,
    ) -> Result<Router, Box<dyn Error + Send + Sync>> {
        let mut env = BTreeMap::new();
        env.insert("CLIENT_SERVICE_TEST".to_owned(), "enabled".to_owned());
        let config = AppConfig::new("127.0.0.1", 0, agent_host_base_url, env);
        let agent_host = AgentHostClient::new(agent_host_base_url, timeout)?;
        Ok(build_router(AppState::with_agent_host(config, agent_host)?))
    }

    fn test_router_with_connection_models_timeout(
        timeout: Duration,
    ) -> Result<Router, Box<dyn Error + Send + Sync>> {
        let mut env = BTreeMap::new();
        env.insert("CLIENT_SERVICE_TEST".to_owned(), "enabled".to_owned());
        let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", env)
            .with_connection_models_timeout(timeout);
        let agent_host = AgentHostClient::new("http://127.0.0.1:9", Duration::from_millis(50))?;
        Ok(build_router(AppState::with_agent_host(config, agent_host)?))
    }

    struct StreamingUpstream {
        base_url: String,
        handle: JoinHandle<Result<(), std::io::Error>>,
    }

    impl StreamingUpstream {
        async fn start(final_delay: Duration) -> Result<Self, Box<dyn Error + Send + Sync>> {
            let app = Router::new()
                .route("/sessions", post(upstream_create_session))
                .route("/sessions/{session_id}", get(upstream_get_session))
                .route("/models", get(upstream_models))
                .route("/skills/{skill_id}/download", get(upstream_download_skill))
                .route(
                    "/sessions/{session_id}/messages/stream",
                    post(upstream_stream_message),
                )
                .with_state(final_delay);
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let handle = tokio::spawn(axum::serve(listener, app).into_future());

            Ok(Self {
                base_url: format_base_url(address),
                handle,
            })
        }
    }

    impl Drop for StreamingUpstream {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    fn format_base_url(address: SocketAddr) -> String {
        format!("http://{address}")
    }

    async fn upstream_create_session(Json(_body): Json<Value>) -> Json<Value> {
        Json(json!({ "session_id": "upstream-session", "status": "idle" }))
    }

    async fn upstream_get_session(Path(session_id): Path<String>) -> Json<Value> {
        Json(json!({ "session_id": session_id, "status": "idle" }))
    }

    async fn upstream_download_skill(Path(skill_id): Path<String>) -> Response {
        (
            [
                (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"SKILL.md\"",
                ),
            ],
            Body::from(format!("# {skill_id}")),
        )
            .into_response()
    }

    async fn upstream_models(State(final_delay): State<Duration>) -> Json<Value> {
        sleep(final_delay).await;
        Json(json!({ "data": [{ "id": "slow-model" }], "object": "list" }))
    }

    async fn upstream_stream_message(
        State(final_delay): State<Duration>,
        Path(_session_id): Path<String>,
        Json(_body): Json<Value>,
    ) -> Response {
        let (sender, receiver) = mpsc::channel::<Result<Vec<u8>, Infallible>>(4);
        tokio::spawn(async move {
            let _sent = sender
                .send(Ok(test_ndjson_line(
                    &json!({ "type": "text_delta", "content": "he" }),
                )))
                .await;
            sleep(final_delay).await;
            let _sent = sender
                .send(Ok(test_ndjson_line(
                    &json!({ "type": "text_delta", "content": "llo" }),
                )))
                .await;
        });
        (
            StatusCode::OK,
            Body::from_stream(ReceiverStream::new(receiver)),
        )
            .into_response()
    }

    fn test_ndjson_line(value: &Value) -> Vec<u8> {
        let mut line = serde_json::to_vec(value).unwrap_or_else(|_error| Vec::from("{}"));
        line.push(b'\n');
        line
    }

    #[tokio::test]
    async fn stream_fanout_drops_backpressured_subscriber_without_blocking()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let (sender, mut receiver) = mpsc::channel(1);
        assert!(
            sender
                .try_send(Ok(test_ndjson_line(&json!({
                    "type": "event",
                    "event": { "type": "text_delta", "content": "queued" }
                }))))
                .is_ok()
        );
        let stream = Arc::new(Mutex::new(ActiveTurnStreamState {
            subscribers: vec![sender],
            final_payload: None,
        }));

        let sent = tokio::time::timeout(Duration::from_millis(50), async {
            send_stream_item(
                &stream,
                &json!({
                    "type": "event",
                    "event": { "type": "text_delta", "content": "blocked" }
                }),
                false,
            )
        })
        .await
        .map_err(|_elapsed| std::io::Error::other("stream fanout blocked"))?;

        assert!(!sent);
        assert!(
            stream
                .lock()
                .map_err(|_error| std::io::Error::other("stream lock poisoned"))?
                .subscribers
                .is_empty()
        );
        let queued = receiver
            .recv()
            .await
            .ok_or_else(|| std::io::Error::other("queued item missing"))??;
        let queued_chunk = serde_json::from_slice::<Value>(&queued)?;
        assert_eq!(queued_chunk["event"]["content"], "queued");
        assert!(receiver.recv().await.is_none());

        Ok(())
    }

    async fn request_json(
        app: Router,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        let mut builder = Request::builder().method(method).uri(path);
        let body = if let Some(body) = body {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&body)?)
        } else {
            Body::empty()
        };
        let response = app.oneshot(builder.body(body)?).await?;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        let value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)?
        };
        Ok((status, value))
    }

    async fn get_json(
        app: Router,
        path: &str,
    ) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        request_json(app, Method::GET, path, None).await
    }

    #[tokio::test]
    async fn health_info_harnesses_and_kernel_configs_work()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let app = test_router()?;

        let (status, value) = get_json(app.clone(), "/healthz").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value, json!({ "status": "ok" }));

        let (status, value) = get_json(app.clone(), "/info").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["client_service"]["service"], "client_service");
        assert_eq!(
            value["client_service"]["env"],
            json!({ "CLIENT_SERVICE_TEST": "enabled" })
        );
        assert_eq!(value["agent_host"]["service"], "agent_host");
        assert!(value["agent_host"]["error"].is_string());

        let (status, value) = get_json(app.clone(), "/harnesses").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            value,
            json!([
                "claude-code",
                "echo",
                "copilot-cli",
                "codex",
                "opencode",
                "acp"
            ])
        );

        let (status, value) = get_json(app.clone(), "/kernel-configs/acp").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["harness"], "acp");
        assert_eq!(value["env_vars"], "");
        assert!(value["updated_at"].is_null());

        let (status, value) = request_json(
            app.clone(),
            Method::PUT,
            "/kernel-configs/acp",
            Some(json!({ "env_vars": "A=B" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["env_vars"], "A=B");

        let (status, value) = get_json(app, "/kernel-configs/unknown").await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(value["detail"].is_string());

        Ok(())
    }

    #[tokio::test]
    async fn info_redacts_secret_env_values() -> Result<(), Box<dyn Error + Send + Sync>> {
        let secret_key_value = "super-secret-master-key-value";
        let api_key_value = "sk-openai-connection-api-key";

        let mut env = BTreeMap::new();
        env.insert("CLIENT_SERVICE_TEST".to_owned(), "enabled".to_owned());
        env.insert(
            "CLIENT_SERVICE_SECRET_KEY".to_owned(),
            secret_key_value.to_owned(),
        );
        env.insert(
            "CLIENT_SERVICE_OPENAI_API_KEY".to_owned(),
            api_key_value.to_owned(),
        );
        let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", env);
        let agent_host = AgentHostClient::new("http://127.0.0.1:9", Duration::from_millis(50))?;
        let app = build_router(AppState::with_agent_host(config, agent_host)?);

        let (status, value) = get_json(app, "/info").await?;
        assert_eq!(status, StatusCode::OK);

        let env = &value["client_service"]["env"];
        assert_eq!(env["CLIENT_SERVICE_TEST"], "enabled");
        assert_eq!(env["CLIENT_SERVICE_SECRET_KEY"], "***redacted***");
        assert_eq!(env["CLIENT_SERVICE_OPENAI_API_KEY"], "***redacted***");

        // The exact secret material must never appear anywhere in the response.
        let serialized = serde_json::to_string(&value)?;
        assert!(!serialized.contains(secret_key_value));
        assert!(!serialized.contains(api_key_value));

        Ok(())
    }

    #[tokio::test]
    async fn connection_routes_handle_crud_and_status_codes()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let app = test_router()?;
        let payload = json!({
            "connection_id": "main",
            "name": "Main",
            "url": "http://models.example.test",
            "api_flavor": "responses",
            "api_key": "secret",
        });

        let (status, value) = request_json(
            app.clone(),
            Method::POST,
            "/connections",
            Some(payload.clone()),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["connection_id"], "main");
        assert_eq!(value["api_key"], "secret");
        assert_eq!(value["has_api_key"], true);

        let (status, value) =
            request_json(app.clone(), Method::POST, "/connections", Some(payload)).await?;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(value["detail"].is_string());

        let (status, value) = get_json(app.clone(), "/connections/main").await?;
        assert_eq!(status, StatusCode::OK);
        assert!(value.get("api_key").is_none());
        assert_eq!(value["api_flavor"], "responses");

        let (status, value) = request_json(
            app.clone(),
            Method::PATCH,
            "/connections/main",
            Some(json!({ "name": "Renamed", "api_key": "" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["name"], "Renamed");
        assert_eq!(value["api_key"], "");
        assert_eq!(value["has_api_key"], false);

        let (status, value) = request_json(
            app.clone(),
            Method::POST,
            "/connections",
            Some(json!({ "connection_id": "Bad", "name": "Bad", "url": "x" })),
        )
        .await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(value["detail"].is_string());

        let (status, value) =
            request_json(app.clone(), Method::DELETE, "/connections/main", None).await?;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(value, Value::Null);

        let (status, value) = get_json(app, "/connections/main").await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(value["detail"].is_string());

        Ok(())
    }

    #[tokio::test]
    async fn connection_models_route_times_out_slow_upstreams()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = StreamingUpstream::start(Duration::from_millis(200)).await?;
        let app = test_router_with_connection_models_timeout(Duration::from_millis(50))?;
        let payload = json!({
            "connection_id": "main",
            "name": "Main",
            "url": upstream.base_url,
        });
        let (status, _value) =
            request_json(app.clone(), Method::POST, "/connections", Some(payload)).await?;
        assert_eq!(status, StatusCode::OK);

        let start = Instant::now();
        let (status, value) = get_json(app, "/connections/main/models").await?;

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(value["detail"].is_string());
        assert!(start.elapsed() < Duration::from_millis(500));

        Ok(())
    }

    #[tokio::test]
    async fn agent_routes_handle_crud_and_missing_connections()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let app = test_router()?;

        let (status, value) = request_json(
            app.clone(),
            Method::POST,
            "/agents",
            Some(json!({
                "agent_id": "agent-one",
                "name": "Agent One",
                "system_prompt": "help",
                "env_vars": "A=B",
                "connection_id": "missing",
            })),
        )
        .await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(value["detail"].is_string());

        let (status, value) = request_json(
            app.clone(),
            Method::POST,
            "/agents",
            Some(json!({ "agent_id": "agent-one", "name": "Agent One" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["harness"], "acp");
        assert_eq!(
            value["system_prompt"],
            crate::models::DEFAULT_AGENT_SYSTEM_PROMPT
        );
        assert_eq!(value["skills"], json!([]));

        let (status, value) = request_json(
            app.clone(),
            Method::PATCH,
            "/agents/agent-one",
            Some(json!({ "name": "Renamed", "harness": "echo" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["name"], "Renamed");
        assert_eq!(value["harness"], "echo");

        let (status, value) = get_json(app.clone(), "/agents").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value.as_array().map(Vec::len), Some(1));

        let (status, _value) =
            request_json(app.clone(), Method::DELETE, "/agents/agent-one", None).await?;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, value) = get_json(app, "/agents/agent-one").await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(value["detail"].is_string());

        Ok(())
    }

    #[tokio::test]
    async fn skill_download_proxies_agent_host_attachment_headers()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = StreamingUpstream::start(Duration::ZERO).await?;
        let app = test_router_with_agent_host(&upstream.base_url, Duration::from_secs(1))?;

        let request = Request::builder()
            .method(Method::GET)
            .uri("/skills/my-skill/download")
            .body(Body::empty())?;
        let response = app.oneshot(request).await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/markdown; charset=utf-8"))
        );
        assert_eq!(
            response.headers().get(header::CONTENT_DISPOSITION),
            Some(&HeaderValue::from_static(
                "attachment; filename=\"SKILL.md\""
            ))
        );
        let body = response.into_body().collect().await?.to_bytes();
        assert_eq!(body.as_ref(), b"# my-skill");

        Ok(())
    }

    #[tokio::test]
    async fn gateway_type_and_stopped_gateway_routes_work()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let app = test_router()?;
        let (status, _value) = request_json(
            app.clone(),
            Method::POST,
            "/agents",
            Some(json!({ "agent_id": "agent-one", "name": "Agent One" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);

        let (status, value) = get_json(app.clone(), "/gateway-types").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value, json!(["echo", "discord"]));

        let (status, value) = get_json(app.clone(), "/gateway-types/echo/schema").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value, json!({ "fields": [] }));

        let (status, value) = get_json(app.clone(), "/gateway-types/discord/schema").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["fields"].as_array().map(Vec::len), Some(5));
        assert_eq!(value["fields"][0]["key"], "DISCORD_BOT_TOKEN");
        assert_eq!(value["fields"][1]["key"], "DISCORD_OWNER_USER_ID");
        assert_eq!(value["fields"][2]["key"], "DISCORD_CHUNK_MAX_CHARS");
        assert_eq!(
            value["fields"][3]["key"],
            "DISCORD_SIMULATED_TYPING_ENABLED"
        );
        assert_eq!(value["fields"][4]["key"], "DISCORD_SIMULATED_TYPING_WPM");
        assert_eq!(value["fields"][0]["kind"], "secret");
        assert_eq!(value["fields"][0]["required"], true);
        assert_eq!(value["fields"][1]["placeholder"], "123456789012345678");

        let (status, value) = get_json(app.clone(), "/gateway-types/not-a-type/schema").await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(value["detail"].is_string());

        let (status, value) = request_json(
            app.clone(),
            Method::POST,
            "/gateways",
            Some(json!({
                "gateway_id": "gateway-one",
                "name": "Gateway One",
                "gateway_type": "echo",
                "agent_id": "agent-one",
                "enabled": false,
                "env_vars": "A=B",
                "secrets": { "TOKEN": "secret" },
            })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["status"], "stopped");
        assert_eq!(value["secret_keys"], json!(["TOKEN"]));
        assert!(value.get("secrets").is_none());

        let (status, value) = request_json(
            app.clone(),
            Method::PATCH,
            "/gateways/gateway-one",
            Some(json!({ "name": "Renamed", "secrets": { "OTHER": "value" } })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["name"], "Renamed");
        assert_eq!(value["secret_keys"], json!(["OTHER", "TOKEN"]));

        let (status, value) =
            request_json(app.clone(), Method::DELETE, "/gateways/gateway-one", None).await?;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(value, Value::Null);

        let (status, value) = get_json(app, "/gateways/gateway-one").await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(value["detail"].is_string());

        Ok(())
    }

    #[tokio::test]
    async fn message_stream_yields_events_before_final_response()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = StreamingUpstream::start(Duration::from_millis(200)).await?;
        let app = test_router_with_agent_host(&upstream.base_url, Duration::from_secs(1))?;

        let (status, _agent) = request_json(
            app.clone(),
            Method::POST,
            "/agents",
            Some(json!({ "agent_id": "agent-one", "name": "Agent One" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let (status, session) = request_json(
            app.clone(),
            Method::POST,
            "/sessions",
            Some(json!({ "agent_id": "agent-one" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let session_id = session
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("session_id missing"))?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/messages/stream"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "message": "hello" }),
            )?))?;
        let started = Instant::now();
        let response =
            tokio::time::timeout(Duration::from_millis(100), app.clone().oneshot(request))
                .await
                .map_err(|_elapsed| {
                    std::io::Error::other("stream response was buffered until upstream completion")
                })??;
        assert!(
            started.elapsed() < Duration::from_millis(150),
            "stream response was not returned promptly"
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/x-ndjson"))
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("no-cache"))
        );
        assert_eq!(
            response
                .headers()
                .get(header::HeaderName::from_static("x-accel-buffering")),
            Some(&HeaderValue::from_static("no"))
        );

        let mut body = response.into_body();
        let first_frame = tokio::time::timeout(Duration::from_millis(100), body.frame())
            .await
            .map_err(|_elapsed| {
                std::io::Error::other("first stream event did not arrive promptly")
            })?
            .ok_or_else(|| std::io::Error::other("stream ended before first event"))??;
        let first_data = first_frame
            .into_data()
            .map_err(|_frame| std::io::Error::other("first frame was not data"))?;
        let first_chunk = serde_json::from_slice::<Value>(&first_data)?;
        assert_eq!(first_chunk["type"], "event");
        assert_eq!(first_chunk["event"]["content"], "he");

        assert!(
            tokio::time::timeout(Duration::from_millis(50), body.frame())
                .await
                .is_err(),
            "final stream item arrived before delayed upstream event"
        );

        let rest = tokio::time::timeout(Duration::from_millis(500), body.collect())
            .await
            .map_err(|_elapsed| {
                std::io::Error::other("stream did not finish after delayed upstream event")
            })??
            .to_bytes();
        let rest_chunks = std::str::from_utf8(&rest)?
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(rest_chunks.len(), 2);
        assert_eq!(rest_chunks[0]["type"], "event");
        assert_eq!(rest_chunks[0]["event"]["content"], "llo");
        assert_eq!(rest_chunks[1]["type"], "final");
        assert_eq!(rest_chunks[1]["assistant_message"]["content"], "hello");
        assert_eq!(rest_chunks[1]["completed"], true);

        let (status, messages) = get_json(app, &format!("/sessions/{session_id}/messages")).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(messages["messages"][1]["content"], "hello");

        Ok(())
    }

    #[tokio::test]
    async fn stream_turn_attaches_to_running_turn_after_disconnect()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = StreamingUpstream::start(Duration::from_millis(200)).await?;
        let app = test_router_with_agent_host(&upstream.base_url, Duration::from_secs(1))?;

        let (status, _agent) = request_json(
            app.clone(),
            Method::POST,
            "/agents",
            Some(json!({ "agent_id": "agent-one", "name": "Agent One" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let (status, session) = request_json(
            app.clone(),
            Method::POST,
            "/sessions",
            Some(json!({ "agent_id": "agent-one" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let session_id = session
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("session_id missing"))?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/messages/stream"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "message": "hello" }),
            )?))?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let first_frame = tokio::time::timeout(Duration::from_millis(100), body.frame())
            .await
            .map_err(|_elapsed| std::io::Error::other("first stream event did not arrive"))?
            .ok_or_else(|| std::io::Error::other("stream ended before first event"))??;
        let first_data = first_frame
            .into_data()
            .map_err(|_frame| std::io::Error::other("first frame was not data"))?;
        let first_chunk = serde_json::from_slice::<Value>(&first_data)?;
        assert_eq!(first_chunk["event"]["content"], "he");
        drop(body);

        let (status, detail) = get_json(app.clone(), &format!("/sessions/{session_id}")).await?;
        assert_eq!(status, StatusCode::OK);
        let active_turn = detail
            .get("active_turn")
            .and_then(Value::as_object)
            .ok_or_else(|| std::io::Error::other("active_turn missing"))?;
        assert_eq!(detail["messages"][1]["content"], "he");
        let turn_id = active_turn
            .get("turn_id")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("turn_id missing"))?;

        let request = Request::builder()
            .method(Method::GET)
            .uri(format!("/sessions/{session_id}/turns/{turn_id}/stream"))
            .body(Body::empty())?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body = tokio::time::timeout(Duration::from_millis(500), response.into_body().collect())
            .await
            .map_err(|_elapsed| std::io::Error::other("attached stream did not finish"))??
            .to_bytes();
        let chunks = std::str::from_utf8(&body)?
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["event"]["content"], "llo");
        assert_eq!(chunks[1]["type"], "final");
        assert_eq!(chunks[1]["assistant_message"]["content"], "hello");

        Ok(())
    }

    #[tokio::test]
    async fn concurrent_message_requests_for_session_are_rejected()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = StreamingUpstream::start(Duration::from_millis(200)).await?;
        let app = test_router_with_agent_host(&upstream.base_url, Duration::from_secs(1))?;

        let (status, _agent) = request_json(
            app.clone(),
            Method::POST,
            "/agents",
            Some(json!({ "agent_id": "agent-one", "name": "Agent One" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let (status, session) = request_json(
            app.clone(),
            Method::POST,
            "/sessions",
            Some(json!({ "agent_id": "agent-one" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let session_id = session
            .get("session_id")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("session_id missing"))?;

        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/messages/stream"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "message": "first" }),
            )?))?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let first_frame = tokio::time::timeout(Duration::from_millis(100), body.frame())
            .await
            .map_err(|_elapsed| std::io::Error::other("first stream event did not arrive"))?
            .ok_or_else(|| std::io::Error::other("stream ended before first event"))??;
        let first_data = first_frame
            .into_data()
            .map_err(|_frame| std::io::Error::other("first frame was not data"))?;
        let first_chunk = serde_json::from_slice::<Value>(&first_data)?;
        assert_eq!(first_chunk["type"], "event");

        for path in [
            format!("/sessions/{session_id}/messages"),
            format!("/sessions/{session_id}/messages/stream"),
        ] {
            let (status, value) = request_json(
                app.clone(),
                Method::POST,
                &path,
                Some(json!({ "message": "second" })),
            )
            .await?;
            assert_eq!(status, StatusCode::CONFLICT);
            assert!(
                value["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("already has active turn"))
            );
        }

        let _rest = tokio::time::timeout(Duration::from_millis(500), body.collect())
            .await
            .map_err(|_elapsed| std::io::Error::other("stream did not finish"))??;

        let (status, messages) =
            get_json(app.clone(), &format!("/sessions/{session_id}/messages")).await?;
        assert_eq!(status, StatusCode::OK);
        let messages = messages["messages"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("messages missing"))?;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["content"], "first");
        assert_eq!(messages[1]["content"], "hello");

        let (status, value) = request_json(
            app,
            Method::POST,
            &format!("/sessions/{session_id}/messages"),
            Some(json!({ "message": "third" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(value["assistant_message"]["content"], "hello");

        Ok(())
    }

    #[tokio::test]
    async fn missing_session_message_and_turn_routes_return_404()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let app = test_router()?;

        let (status, value) = get_json(app.clone(), "/sessions/missing/messages").await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(value["detail"].is_string());

        let (status, value) = get_json(app, "/sessions/missing/turns/turn-one/stream").await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(value["detail"].is_string());

        Ok(())
    }
}
