use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{OriginalUri, Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{sync::mpsc, time::sleep};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::{
    ActiveTurnRecord, ActiveTurnStreamState, AppState, ENV_PREFIX, StreamItem,
    agent_host::{AgentHostError, JsonObject, KernelEvent},
    errors::{StoreError, ValidationError},
    git_agent::GitAgentError,
    memory::{MEMORY_JSON_CONTENT_TYPE, MEMORY_RUN_CONTENT_TYPE, MemoryProxyError},
    models::{
        AgentRecord, BUILTIN_GIT_AGENT_WORKSPACE_ID, BUILTIN_GIT_AGENT_WORKSPACE_NAME, ClientType,
        ConnectionApiFlavor, ConnectionRecord, DEFAULT_AGENT_SYSTEM_PROMPT,
        DEFAULT_GIT_AGENT_REVIEW_AGENT_ID, GatewayRecord, GatewayType, GitAgentConfigRecord,
        HarnessName, MessageRecord, MessageRole, SessionRecord, ToolCallRecord,
        WorkspaceMountRecord, WorkspaceRecord, WorkspaceStatus, parse_env_vars, utc_now,
        validate_agent_id, validate_connection_id, validate_gateway_id, validate_skill_id,
        validate_workspace_id,
    },
};

const DEFAULT_AGENTSPACE_CLIENT_SERVICE_URL: &str = "http://client-service:8002";
const AGENTSPACE_CLIENT_SERVICE_URL_ENV: &str = "CLIENT_SERVICE_AGENTSPACE_BASE_URL";
const GATEWAY_AUTOSTART_ATTEMPTS: usize = 5;
const GATEWAY_AUTOSTART_RETRY_DELAY: Duration = Duration::from_secs(2);
const MEMORY_MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MEMORY_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

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
        .merge(git_agent_router())
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
        .merge(session_control_router())
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
}

fn session_control_router() -> Router<AppState> {
    Router::new().route(
        "/internal/session-control/start-new",
        post(request_start_new_session),
    )
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

fn git_agent_router() -> Router<AppState> {
    Router::new()
        .route(
            "/git-agent/config",
            get(get_git_agent_config).put(update_git_agent_config),
        )
        .route("/git-agent/status", get(git_agent_status))
        .route("/git-agent/requests", get(list_git_agent_requests))
        .route(
            "/git-agent/requests/{request_id}",
            get(get_git_agent_request),
        )
        .route(
            "/git-agent/requests/{request_id}/rerun-review",
            post(rerun_git_agent_review),
        )
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
            "env": state.config.client_service_env,
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
        has_api_key = !connection.api_key.is_empty(),
        "api handler completed"
    );
    Ok(Json(connection.summary(false)))
}

async fn list_connection_models(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let connection = require_connection(&state, &connection_id)?;
    tracing::info!(
        route = "/connections/:connection_id/models",
        action = "list_connection_models",
        connection_id = %connection_id,
        api_flavor = connection.api_flavor.as_str(),
        "fetching connection models"
    );
    let url = format!("{}/models", connection.url.trim_end_matches('/'));
    let mut request = state
        .http_client
        .get(url)
        .timeout(state.config.connection_models_timeout());
    if !connection.api_key.is_empty() {
        request = request.bearer_auth(&connection.api_key);
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
    connection.api_key = payload.api_key;
    let value = connection.summary(true);
    state.connections.insert(connection)?;
    tracing::info!(
        route = "/connections",
        action = "create_connection",
        connection_id = %value["connection_id"].as_str().unwrap_or_default(),
        api_flavor = %value["api_flavor"].as_str().unwrap_or_default(),
        has_api_key = value["has_api_key"].as_bool().unwrap_or(false),
        "api handler completed"
    );
    Ok(Json(value))
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
    if let Some(api_key) = payload.api_key {
        connection.api_key = api_key;
    }
    connection.updated_at = utc_now();
    let value = connection.summary(true);
    state.connections.update(connection)?;
    tracing::info!(
        route = "/connections/:connection_id",
        action = "update_connection",
        connection_id = %connection_id,
        api_flavor = %value["api_flavor"].as_str().unwrap_or_default(),
        has_api_key = value["has_api_key"].as_bool().unwrap_or(false),
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
    if let Some(connection_id) = payload.connection_id.as_deref() {
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
    validate_workspace_mounts(&state, &payload.workspace_mounts)?;
    agent.workspace_mounts = payload.workspace_mounts;
    let value = agent.summary();
    state.agents.insert(agent)?;
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
    agent.updated_at = utc_now();
    let value = agent.summary();
    state.agents.update(agent)?;
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
        state
            .agent_host
            .destroy_session(&session.agent_host_session_id)
            .await?;
    }
    tracing::info!(
        route = "/agents/:agent_id",
        action = "delete_agent",
        agent_id = %agent_id,
        "api handler completed"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn get_git_agent_config(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let config = get_or_init_git_agent_config(&state)?;
    ensure_git_agent_review_agent(&state, &config.review_agent_id)?;
    tracing::info!(
        route = "/git-agent/config",
        action = "get_git_agent_config",
        enabled = config.enabled,
        default_branch = %config.default_branch,
        review_agent_id = %config.review_agent_id,
        "api handler completed"
    );
    Ok(Json(config.summary()))
}

async fn update_git_agent_config(
    State(state): State<AppState>,
    Json(payload): Json<UpdateGitAgentConfigRequest>,
) -> Result<Json<Value>, ApiError> {
    let mut config = get_or_init_git_agent_config(&state)?;
    apply_git_agent_config_update(&state, &mut config, payload)?;
    config.updated_at = utc_now();
    let config = state.git_agent_config.upsert(config)?;
    ensure_git_agent_review_agent(&state, &config.review_agent_id)?;
    tracing::info!(
        route = "/git-agent/config",
        action = "update_git_agent_config",
        enabled = config.enabled,
        default_branch = %config.default_branch,
        review_agent_id = %config.review_agent_id,
        allowed_ref_prefix_count = config.allowed_ref_prefixes.len(),
        allowed_ref_count = config.allowed_refs.len(),
        "api handler completed"
    );
    Ok(Json(config.summary()))
}

async fn git_agent_status(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let status = state.git_agent.status().await?;
    tracing::info!(
        route = "/git-agent/status",
        action = "git_agent_status",
        "api handler completed"
    );
    Ok(Json(Value::Object(status)))
}

async fn list_git_agent_requests(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let requests = state.git_agent.list_requests().await?;
    let request_count = requests.as_array().map_or(0, Vec::len);
    tracing::info!(
        route = "/git-agent/requests",
        action = "list_git_agent_requests",
        request_count,
        "api handler completed"
    );
    Ok(Json(requests))
}

async fn get_git_agent_request(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let request = state.git_agent.get_request(&request_id).await?;
    tracing::info!(
        route = "/git-agent/requests/:request_id",
        action = "get_git_agent_request",
        request_id = %request_id,
        has_raw_patch = request.get("raw_patch").is_some(),
        "api handler completed"
    );
    Ok(Json(Value::Object(request)))
}

async fn rerun_git_agent_review(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let request = state.git_agent.rerun_review(&request_id).await?;
    tracing::info!(
        route = "/git-agent/requests/:request_id/rerun-review",
        action = "rerun_git_agent_review",
        request_id = %request_id,
        "api handler completed"
    );
    Ok(Json(Value::Object(request)))
}

async fn list_workspaces(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let mut workspaces = vec![builtin_git_agent_workspace_summary(&state)];
    workspaces.extend(
        state
            .workspaces
            .list()?
            .into_iter()
            .filter(|workspace| !is_builtin_workspace_id(&workspace.workspace_id))
            .map(|workspace| workspace.summary()),
    );
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
    reject_builtin_workspace_mutation(&payload.workspace_id)?;
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
    if is_builtin_workspace_id(&workspace_id) {
        tracing::info!(
            route = "/workspaces/:workspace_id",
            action = "get_workspace",
            workspace_id = %workspace_id,
            builtin = true,
            "api handler completed"
        );
        return Ok(Json(builtin_git_agent_workspace_summary(&state)));
    }
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
    reject_builtin_workspace_mutation(&workspace_id)?;
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
    reject_builtin_workspace_mutation(&workspace_id)?;
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
    reject_builtin_workspace_mutation(&source_workspace_id)?;
    validate_workspace_id(&payload.workspace_id)?;
    reject_builtin_workspace_mutation(&payload.workspace_id)?;
    let source_workspace = require_ready_workspace(&state, &source_workspace_id)?;
    let mut target_workspace = WorkspaceRecord::new_with_status(
        payload.workspace_id,
        payload.name,
        WorkspaceStatus::Creating,
    );
    let target_workspace_id = target_workspace.workspace_id.clone();
    let target_volume_name = target_workspace.volume_name();
    state.workspaces.insert(target_workspace.clone())?;
    let clone_result = state
        .agent_host
        .clone_workspace(
            &source_workspace.volume_name(),
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
    let (workspace_id_for_editor, volume_name) = if is_builtin_workspace_id(&workspace_id) {
        (
            BUILTIN_GIT_AGENT_WORKSPACE_ID.to_owned(),
            state.config.git_agent_data_volume_name().to_owned(),
        )
    } else {
        let workspace = require_ready_workspace(&state, &workspace_id)?;
        (workspace.workspace_id.clone(), workspace.volume_name())
    };
    let upstream = state
        .agent_host
        .open_workspace_vscode(&workspace_id_for_editor, &volume_name)
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
    let session_id = Uuid::now_v7().simple().to_string();
    let session_control_token = generate_session_control_token()?;
    let env = session_env(&state, &agent, &session_id, &session_control_token)?;
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
    let workspace_mounts = agent_host_workspace_mounts(&state, &session_mounts);
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
    let mut session = SessionRecord::new(
        session_id,
        payload.agent_id,
        upstream_session_id.clone(),
        status.clone(),
        payload.channel_name,
        payload.client_type,
    );
    session.session_control_token_hash = Some(hash_session_control_token(&session_control_token));
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

async fn request_start_new_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<StartNewSessionRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let token = bearer_token(&headers)?;
    let session = authenticate_session_control(&state, &payload.session_id, token)?;
    let mut active_turns = state
        .active_turns
        .lock()
        .map_err(|_error| ApiError::internal("active turn lock poisoned".to_owned()))?;
    let turn = active_turns
        .get_mut(&session.session_id)
        .ok_or_else(|| ApiError::conflict("session does not have an active turn".to_owned()))?;
    if turn.automatic_restart_count > 0 {
        return Err(ApiError::conflict(
            "fresh-session handoff already occurred for this turn".to_owned(),
        ));
    }
    if turn.restart_requests_closed {
        return Err(ApiError::conflict(
            "active turn is already completing".to_owned(),
        ));
    }
    turn.start_new_requested = true;
    let turn_id = turn.turn_id.clone();
    drop(active_turns);

    tracing::info!(
        route = "/internal/session-control/start-new",
        action = "request_start_new_session",
        session_id = %session.session_id,
        turn_id = %turn_id,
        "fresh-session handoff accepted"
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "accepted": true,
            "turn_id": turn_id,
        })),
    ))
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
    let session = require_session(&state, &session_id)?;
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
    let session = require_session(&state, &session_id)?;
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
    let session = reset_session_internal(&state, &session_id).await?;
    Ok(Json(session_summary(&state, &session)?))
}

async fn reset_session_internal(
    state: &AppState,
    session_id: &str,
) -> Result<SessionRecord, ApiError> {
    let mut session = require_session(state, session_id)?;
    let upstream = state
        .agent_host
        .reset_session(&session.agent_host_session_id)
        .await?;
    session.agent_host_session_id = string_field(&upstream, "session_id")?;
    session.status = string_field(&upstream, "status")?;
    session.updated_at = utc_now();
    state.sessions.clear_messages(session_id)?;
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
    Ok(session)
}

async fn save_session_workspace(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SaveSessionWorkspaceRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_workspace_id(&payload.workspace_id)?;
    let session = require_session(&state, &session_id)?;
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
    state
        .agent_host
        .destroy_session(&session.agent_host_session_id)
        .await?;
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
    if let Some(creator_agent_id) = payload.creator_agent_id.as_deref() {
        validate_agent_id(creator_agent_id)?;
        require_agent(&state, creator_agent_id)?;
    }
    let skill = state
        .agent_host
        .create_skill(&payload.skill_id, &payload.files)
        .await?;
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
    let skills = state
        .agent_host
        .list_skills()
        .await?
        .into_iter()
        .map(Value::Object)
        .collect::<Vec<_>>();
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
    let skill = state.agent_host.get_skill(&skill_id).await?;
    tracing::info!(
        route = "/skills/:skill_id",
        action = "get_skill",
        skill_id = %skill_id,
        "api handler completed"
    );
    Ok(Json(Value::Object(skill)))
}

async fn download_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<Response, ApiError> {
    validate_skill_id(&skill_id)?;
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
    let skill = state
        .agent_host
        .update_skill(&skill_id, &payload.files)
        .await?;
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
    let skill = state
        .agent_host
        .rollback_skill_version(&skill_id, version)
        .await?;
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
    state.agent_host.delete_skill(&skill_id).await?;
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
    state.gateways.insert(gateway)?;
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
    state.gateways.update(gateway)?;
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

async fn start_gateway_by_id(
    state: &AppState,
    gateway_id: &str,
    failure_mode: GatewayStartFailureMode,
) -> Result<Value, ApiError> {
    let mut gateway = require_gateway(state, gateway_id)?;
    "starting".clone_into(&mut gateway.status);
    gateway.last_error = None;
    gateway.updated_at = utc_now();
    state.gateways.update(gateway.clone())?;
    let env = gateway.effective_env();
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
            gateway.updated_at = utc_now();
            state.gateways.update(gateway.clone())?;
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
            gateway.updated_at = utc_now();
            state.gateways.update(gateway.clone())?;
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
    gateway.updated_at = utc_now();
    state.gateways.update(gateway.clone())?;
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

fn generate_session_control_token() -> Result<String, ApiError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        ApiError::internal(format!("failed to generate session capability: {error}"))
    })?;
    Ok(hex_encode(&bytes))
}

fn hash_session_control_token(token: &str) -> String {
    hex_encode(&Sha256::digest(token.as_bytes()))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _written = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_session_control_credentials)
}

fn authenticate_session_control(
    state: &AppState,
    session_id: &str,
    token: &str,
) -> Result<SessionRecord, ApiError> {
    let session = state
        .sessions
        .get(session_id)?
        .ok_or_else(invalid_session_control_credentials)?;
    let expected_hash = session
        .session_control_token_hash
        .as_deref()
        .ok_or_else(invalid_session_control_credentials)?;
    let actual_hash = hash_session_control_token(token);
    if !bool::from(actual_hash.as_bytes().ct_eq(expected_hash.as_bytes())) {
        return Err(invalid_session_control_credentials());
    }
    Ok(session)
}

fn invalid_session_control_credentials() -> ApiError {
    ApiError::unauthorized("invalid session control credentials".to_owned())
}

fn session_env(
    state: &AppState,
    agent: &AgentRecord,
    session_id: &str,
    session_control_token: &str,
) -> Result<BTreeMap<String, String>, ApiError> {
    let mut env = BTreeMap::new();
    if let Some(config) = state.kernel_configs.get(agent.harness)? {
        env.extend(parse_env_vars(&config.env_vars));
    }
    if let Some(connection_id) = agent.connection_id.as_deref() {
        let connection = require_connection(state, connection_id)?;
        env.insert("CONNECTION_URL".to_owned(), connection.url);
        env.insert(
            "CONNECTION_API_FLAVOR".to_owned(),
            connection.api_flavor.as_str().to_owned(),
        );
        if !connection.api_key.is_empty() {
            env.insert("CONNECTION_API_KEY".to_owned(), connection.api_key);
        }
    }
    env.extend(parse_env_vars(&agent.env_vars));
    env.insert("AGENTSPACE_AGENT_ID".to_owned(), agent.agent_id.clone());
    env.insert("AGENTSPACE_SESSION_ID".to_owned(), session_id.to_owned());
    env.insert(
        "AGENTSPACE_SESSION_CONTROL_TOKEN".to_owned(),
        session_control_token.to_owned(),
    );
    env.insert(
        "AGENTSPACE_CLIENT_SERVICE_URL".to_owned(),
        state
            .config
            .client_service_env
            .get(AGENTSPACE_CLIENT_SERVICE_URL_ENV)
            .cloned()
            .unwrap_or_else(|| DEFAULT_AGENTSPACE_CLIENT_SERVICE_URL.to_owned()),
    );
    if !agent.system_prompt.is_empty() {
        env.insert(
            "KERNEL_SYSTEM_PROMPT".to_owned(),
            agent.system_prompt.clone(),
        );
    }
    tracing::debug!(
        action = "session_env",
        agent_id = %agent.agent_id,
        harness = agent.harness.as_str(),
        env_var_count = env.len(),
        has_connection = agent.connection_id.is_some(),
        has_system_prompt = !agent.system_prompt.is_empty(),
        "session environment prepared"
    );
    Ok(env)
}

struct TurnExecution {
    turn_id: String,
    session_id: String,
    agent_host_session_id: String,
    message: String,
    user_message_id: String,
    assistant_message_id: String,
    assistant_created_at: String,
    stream: Option<Arc<Mutex<ActiveTurnStreamState>>>,
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
) -> Result<(TurnExecution, NdjsonReceiver), ApiError> {
    let (turn, receiver) = start_turn(state, session_id, message, true)?;
    let receiver =
        receiver.ok_or_else(|| ApiError::internal("streaming turn receiver missing".to_owned()))?;
    Ok((turn, receiver))
}

fn start_turn(
    state: &AppState,
    session_id: &str,
    message: String,
    streaming: bool,
) -> Result<(TurnExecution, Option<NdjsonReceiver>), ApiError> {
    let mut session = require_session(state, session_id)?;
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
    let active_stream = streaming.then(|| stream.clone());
    let active_turn = begin_active_turn(
        state,
        &session.session_id,
        ActiveTurnRecord {
            turn_id: turn_id.clone(),
            user_message_id: user_message.message_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
            start_new_requested: false,
            automatic_restart_count: 0,
            restart_requests_closed: false,
            stream: active_stream,
        },
    )?;
    let receiver = streaming
        .then(|| subscribe_stream_state(&stream))
        .transpose()?;

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
        TurnExecution {
            turn_id,
            session_id: session.session_id,
            agent_host_session_id: session.agent_host_session_id,
            message,
            user_message_id: user_message.message_id,
            assistant_message_id,
            assistant_created_at,
            stream: streaming.then_some(stream),
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

struct TurnOutcome {
    events: Vec<KernelEvent>,
    completed: bool,
    error: Option<String>,
    error_status: Option<StatusCode>,
    automatic_restart_count: u8,
}

enum RestartHandling {
    NotRequested,
    Restarted,
    Failed,
}

async fn run_streaming_turn(state: AppState, mut turn: TurnExecution) {
    let outcome = execute_turn(&state, &mut turn).await;
    let final_payload = match finalize_turn(&state, &turn, &outcome, true).await {
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
                "turn_id": turn.turn_id,
                "completed": false,
                "error": finalize_error.detail,
            })
        }
    };
    let sent = send_turn_item(&turn, &final_payload, true);
    tracing::info!(
        action = "run_streaming_turn",
        session_id = %turn.session_id,
        turn_id = %turn.turn_id,
        kernel_session_id = %turn.agent_host_session_id,
        completed = outcome.completed,
        has_error = outcome.error.is_some(),
        event_count = outcome.events.len(),
        automatic_restart_count = outcome.automatic_restart_count,
        final_sent = sent,
        "streaming turn finished"
    );
}

async fn execute_turn(state: &AppState, turn: &mut TurnExecution) -> TurnOutcome {
    let mut outcome = TurnOutcome {
        events: Vec::new(),
        completed: false,
        error: None,
        error_status: None,
        automatic_restart_count: 0,
    };

    'attempt: loop {
        tracing::info!(
            action = "execute_turn",
            session_id = %turn.session_id,
            turn_id = %turn.turn_id,
            kernel_session_id = %turn.agent_host_session_id,
            automatic_restart_count = outcome.automatic_restart_count,
            "upstream stream starting"
        );
        let stream_result = state
            .agent_host
            .stream_message(&turn.agent_host_session_id, &turn.message)
            .await;
        let mut stream = match stream_result {
            Ok(stream) => stream,
            Err(stream_error) => {
                match handle_restart_request(state, turn, &mut outcome, true).await {
                    RestartHandling::Restarted => continue 'attempt,
                    RestartHandling::Failed => break,
                    RestartHandling::NotRequested => {}
                }
                let error = ApiError::from(stream_error);
                outcome.error_status = Some(error.status);
                outcome.error = Some(error.detail);
                break;
            }
        };

        loop {
            match stream.next_event().await {
                Ok(Some(event)) => {
                    if restart_requested(state, turn) {
                        drop(stream);
                        match handle_restart_request(state, turn, &mut outcome, false).await {
                            RestartHandling::Restarted => continue 'attempt,
                            RestartHandling::NotRequested | RestartHandling::Failed => {
                                break 'attempt;
                            }
                        }
                    }
                    if let Err(error) = process_turn_event(state, turn, &mut outcome.events, event)
                    {
                        match handle_restart_request(state, turn, &mut outcome, true).await {
                            RestartHandling::Restarted => continue 'attempt,
                            RestartHandling::Failed => break 'attempt,
                            RestartHandling::NotRequested => {}
                        }
                        outcome.error_status = Some(error.status);
                        outcome.error = Some(error.detail);
                        break 'attempt;
                    }
                }
                Ok(None) => {
                    match handle_restart_request(state, turn, &mut outcome, true).await {
                        RestartHandling::Restarted => continue 'attempt,
                        RestartHandling::Failed => break 'attempt,
                        RestartHandling::NotRequested => {}
                    }
                    outcome.completed = true;
                    break 'attempt;
                }
                Err(stream_error) => {
                    match handle_restart_request(state, turn, &mut outcome, true).await {
                        RestartHandling::Restarted => continue 'attempt,
                        RestartHandling::Failed => break 'attempt,
                        RestartHandling::NotRequested => {}
                    }
                    let error = ApiError::from(stream_error);
                    outcome.error_status = Some(error.status);
                    outcome.error = Some(error.detail);
                    break 'attempt;
                }
            }
        }
    }

    outcome
}

fn restart_requested(state: &AppState, turn: &TurnExecution) -> bool {
    state
        .active_turns
        .lock()
        .ok()
        .and_then(|active_turns| active_turns.get(&turn.session_id).cloned())
        .is_some_and(|active| active.turn_id == turn.turn_id && active.start_new_requested)
}

async fn handle_restart_request(
    state: &AppState,
    turn: &mut TurnExecution,
    outcome: &mut TurnOutcome,
    close_if_absent: bool,
) -> RestartHandling {
    let requested = match take_restart_request(state, turn, close_if_absent) {
        Ok(requested) => requested,
        Err(error) => {
            outcome.error_status = Some(error.status);
            outcome.error = Some(error.detail);
            return RestartHandling::Failed;
        }
    };
    if !requested {
        return RestartHandling::NotRequested;
    }

    outcome.automatic_restart_count = 1;
    outcome.events.clear();
    match restart_turn(state, turn).await {
        Ok(restart_event) => {
            outcome.events.push(restart_event.clone());
            send_turn_event(turn, &restart_event);
            RestartHandling::Restarted
        }
        Err(error) => {
            outcome.error_status = Some(error.status);
            outcome.error = Some(error.detail);
            RestartHandling::Failed
        }
    }
}

fn take_restart_request(
    state: &AppState,
    turn: &TurnExecution,
    close_if_absent: bool,
) -> Result<bool, ApiError> {
    let mut active_turns = state
        .active_turns
        .lock()
        .map_err(|_error| ApiError::internal("active turn lock poisoned".to_owned()))?;
    let active = active_turns
        .get_mut(&turn.session_id)
        .filter(|active| active.turn_id == turn.turn_id)
        .ok_or_else(|| ApiError::internal("active turn disappeared during execution".to_owned()))?;
    let requested = if !active.start_new_requested {
        if close_if_absent {
            active.restart_requests_closed = true;
        }
        false
    } else if active.automatic_restart_count > 0 {
        active.start_new_requested = false;
        false
    } else {
        active.start_new_requested = false;
        active.automatic_restart_count = 1;
        true
    };
    drop(active_turns);
    Ok(requested)
}

async fn restart_turn(state: &AppState, turn: &mut TurnExecution) -> Result<KernelEvent, ApiError> {
    let reset_result = reset_session_internal(state, &turn.session_id).await;
    let new_session = match reset_result {
        Ok(session) => session,
        Err(error) => {
            replace_turn_messages(state, turn)?;
            return Err(error);
        }
    };
    turn.agent_host_session_id = new_session.agent_host_session_id;
    replace_turn_messages(state, turn)?;

    tracing::info!(
        action = "restart_turn",
        session_id = %turn.session_id,
        turn_id = %turn.turn_id,
        kernel_session_id = %turn.agent_host_session_id,
        automatic_restart_count = 1,
        "fresh-session handoff completed; replay starting"
    );
    Ok([
        ("type".to_owned(), json!("agentspace/session-restarted")),
        ("restart_count".to_owned(), json!(1)),
    ]
    .into_iter()
    .collect())
}

fn replace_turn_messages(state: &AppState, turn: &mut TurnExecution) -> Result<(), ApiError> {
    state.sessions.clear_messages(&turn.session_id)?;
    let user_message = MessageRecord::new(
        Uuid::now_v7().simple().to_string(),
        turn.session_id.clone(),
        MessageRole::User,
        turn.message.clone(),
    );
    let assistant_message = MessageRecord::new(
        Uuid::now_v7().simple().to_string(),
        turn.session_id.clone(),
        MessageRole::Assistant,
        "",
    );
    turn.user_message_id.clone_from(&user_message.message_id);
    turn.assistant_message_id
        .clone_from(&assistant_message.message_id);
    turn.assistant_created_at
        .clone_from(&assistant_message.created_at);
    state.sessions.append_message(&user_message)?;
    state.sessions.append_message(&assistant_message)?;

    let mut active_turns = state
        .active_turns
        .lock()
        .map_err(|_error| ApiError::internal("active turn lock poisoned".to_owned()))?;
    let active = active_turns
        .get_mut(&turn.session_id)
        .filter(|active| active.turn_id == turn.turn_id)
        .ok_or_else(|| ApiError::internal("active turn disappeared during restart".to_owned()))?;
    turn.user_message_id.clone_into(&mut active.user_message_id);
    turn.assistant_message_id
        .clone_into(&mut active.assistant_message_id);
    drop(active_turns);
    Ok(())
}

fn process_turn_event(
    state: &AppState,
    turn: &TurnExecution,
    events: &mut Vec<KernelEvent>,
    event: KernelEvent,
) -> Result<(), ApiError> {
    events.push(event);
    let assistant_message = assistant_message_from_events(
        &turn.session_id,
        &turn.assistant_message_id,
        &turn.assistant_created_at,
        events,
    );
    state.sessions.update_message(&assistant_message)?;
    if let Some(event) = events.last() {
        send_turn_event(turn, event);
    }
    Ok(())
}

fn send_turn_event(turn: &TurnExecution, event: &KernelEvent) {
    let _sent = send_turn_item(
        turn,
        &json!({
            "type": "event",
            "event": Value::Object(event.clone()),
        }),
        false,
    );
}

fn send_turn_item(turn: &TurnExecution, value: &Value, close: bool) -> bool {
    turn.stream
        .as_ref()
        .is_some_and(|stream| send_stream_item(stream, value, close))
}

async fn finalize_turn(
    state: &AppState,
    turn: &TurnExecution,
    outcome: &TurnOutcome,
    streaming: bool,
) -> Result<Value, ApiError> {
    let assistant_message = assistant_message_from_events(
        &turn.session_id,
        &turn.assistant_message_id,
        &turn.assistant_created_at,
        &outcome.events,
    );
    state.sessions.update_message(&assistant_message.clone())?;

    let mut session = require_session(state, &turn.session_id)?;
    if let Ok(upstream) = state
        .agent_host
        .get_session(&turn.agent_host_session_id)
        .await
        && let Ok(status) = string_field(&upstream, "status")
    {
        session.status = status;
    }
    if outcome.error.is_some() {
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
        completed = outcome.completed,
        has_error = outcome.error.is_some(),
        event_count = outcome.events.len(),
        automatic_restart_count = outcome.automatic_restart_count,
        tool_call_count = assistant_message.tool_calls.len(),
        "streaming turn finalized"
    );

    let mut payload = json!({
        "session": session.summary(),
        "assistant_message": assistant_message.summary(),
        "events": outcome.events,
        "turn_id": turn.turn_id,
        "completed": outcome.completed,
    });
    if let Value::Object(object) = &mut payload {
        if streaming {
            object.insert("type".to_owned(), json!("final"));
        }
        if let Some(error) = &outcome.error {
            object.insert("error".to_owned(), json!(error));
        }
        if outcome.automatic_restart_count > 0 {
            object.insert(
                "automatic_restart_count".to_owned(),
                json!(outcome.automatic_restart_count),
            );
        }
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
    let (mut turn, receiver) = start_turn(state, session_id, message.to_owned(), false)?;
    debug_assert!(receiver.is_none());
    tracing::info!(
        action = "run_turn",
        session_id = %turn.session_id,
        turn_id = %turn.turn_id,
        kernel_session_id = %turn.agent_host_session_id,
        message_char_count = message.chars().count(),
        "synchronous turn started"
    );
    let outcome = execute_turn(state, &mut turn).await;
    if outcome.automatic_restart_count == 0
        && let Some(error) = &outcome.error
    {
        return Err(ApiError {
            status: outcome
                .error_status
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            detail: error.clone(),
        });
    }
    finalize_turn(state, &turn, &outcome, false).await
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

fn builtin_git_agent_workspace_summary(state: &AppState) -> Value {
    json!({
        "workspace_id": BUILTIN_GIT_AGENT_WORKSPACE_ID,
        "name": BUILTIN_GIT_AGENT_WORKSPACE_NAME,
        "status": WorkspaceStatus::Ready.as_str(),
        "mount_path": format!("/workspace/{BUILTIN_GIT_AGENT_WORKSPACE_ID}"),
        "volume_name": state.config.git_agent_data_volume_name(),
        "builtin": true,
        "created_at": "1970-01-01T00:00:00Z",
        "updated_at": "1970-01-01T00:00:00Z",
    })
}

fn is_builtin_workspace_id(workspace_id: &str) -> bool {
    workspace_id == BUILTIN_GIT_AGENT_WORKSPACE_ID
}

fn reject_builtin_workspace_mutation(workspace_id: &str) -> Result<(), ApiError> {
    if is_builtin_workspace_id(workspace_id) {
        return Err(ApiError::conflict(
            "git-agent is a built-in workspace and cannot be modified".to_owned(),
        ));
    }
    Ok(())
}

fn agent_host_workspace_mounts(
    state: &AppState,
    mounts: &[WorkspaceMountRecord],
) -> Vec<WorkspaceMountRecord> {
    mounts
        .iter()
        .map(|mount| {
            if !is_builtin_workspace_id(&mount.workspace_id) {
                return mount.clone();
            }
            let mut mount = mount.clone();
            mount.volume_name = Some(state.config.git_agent_data_volume_name().to_owned());
            mount
        })
        .collect()
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

fn get_or_init_git_agent_config(state: &AppState) -> Result<GitAgentConfigRecord, ApiError> {
    if let Some(config) = state.git_agent_config.get()? {
        return Ok(config);
    }
    let config = GitAgentConfigRecord::new_default();
    state.git_agent_config.upsert(config).map_err(Into::into)
}

fn apply_git_agent_config_update(
    state: &AppState,
    config: &mut GitAgentConfigRecord,
    payload: UpdateGitAgentConfigRequest,
) -> Result<(), ApiError> {
    if let Some(enabled) = payload.enabled {
        config.enabled = enabled;
    }
    if let Some(default_branch) = payload.default_branch {
        validate_git_branch(&default_branch)?;
        config.default_branch = default_branch;
    }
    if let Some(allowed_ref_prefixes) = payload.allowed_ref_prefixes {
        for prefix in &allowed_ref_prefixes {
            validate_git_ref_prefix(prefix)?;
        }
        config.allowed_ref_prefixes = allowed_ref_prefixes;
    }
    if let Some(allowed_refs) = payload.allowed_refs {
        for git_ref in &allowed_refs {
            validate_git_ref(git_ref)?;
        }
        config.allowed_refs = allowed_refs;
    }
    if let Some(remote_url) = payload.remote_url {
        validate_non_empty_field("remote_url", &remote_url)?;
        config.remote_url = remote_url;
    }
    if let Some(patch_url) = payload.patch_url {
        validate_non_empty_field("patch_url", &patch_url)?;
        config.patch_url = patch_url;
    }
    if let Some(review_agent_id) = payload.review_agent_id {
        validate_agent_id(&review_agent_id)?;
        if review_agent_id == DEFAULT_GIT_AGENT_REVIEW_AGENT_ID {
            ensure_git_agent_review_agent(state, &review_agent_id)?;
        } else {
            require_agent(state, &review_agent_id)?;
        }
        config.review_agent_id = review_agent_id;
    } else if config.review_agent_id == DEFAULT_GIT_AGENT_REVIEW_AGENT_ID {
        ensure_git_agent_review_agent(state, &config.review_agent_id)?;
    } else {
        require_agent(state, &config.review_agent_id)?;
    }
    if let Some(validation_command) = payload.validation_command {
        config.validation_command = validation_command;
    }
    Ok(())
}

fn ensure_git_agent_review_agent(state: &AppState, agent_id: &str) -> Result<(), ApiError> {
    if agent_id != DEFAULT_GIT_AGENT_REVIEW_AGENT_ID || state.agents.get(agent_id)?.is_some() {
        return Ok(());
    }
    state.agents.insert(default_git_agent_review_agent())?;
    tracing::info!(
        action = "ensure_git_agent_review_agent",
        agent_id,
        "created default git agent reviewer"
    );
    Ok(())
}

fn default_git_agent_review_agent() -> AgentRecord {
    AgentRecord::new(
        DEFAULT_GIT_AGENT_REVIEW_AGENT_ID,
        "Git Agent Reviewer",
        HarnessName::Acp,
        "Review submitted patches for correctness, safety, and repository policy before GitAgent commits them.",
    )
}

fn validate_git_branch(value: &str) -> Result<(), ApiError> {
    validate_non_empty_field("default_branch", value)?;
    validate_git_name_component("default_branch", value)
}

fn validate_git_ref(value: &str) -> Result<(), ApiError> {
    validate_non_empty_field("allowed_refs", value)?;
    if !value.starts_with("refs/") {
        return Err(ApiError::unprocessable(format!(
            "allowed_refs entries must start with refs/, got {value:?}"
        )));
    }
    validate_git_name_component("allowed_refs", value)
}

fn validate_git_ref_prefix(value: &str) -> Result<(), ApiError> {
    validate_non_empty_field("allowed_ref_prefixes", value)?;
    if !value.starts_with("refs/") {
        return Err(ApiError::unprocessable(format!(
            "allowed_ref_prefixes entries must start with refs/, got {value:?}"
        )));
    }
    if !value.ends_with('/') {
        return Err(ApiError::unprocessable(format!(
            "allowed_ref_prefixes entries must end with '/', got {value:?}"
        )));
    }
    validate_git_name_component("allowed_ref_prefixes", value)
}

fn validate_git_name_component(field: &'static str, value: &str) -> Result<(), ApiError> {
    if value.contains(char::is_whitespace) || value.contains("..") {
        return Err(ApiError::unprocessable(format!(
            "{field} contains an invalid git ref component: {value:?}"
        )));
    }
    Ok(())
}

fn validate_non_empty_field(field: &'static str, value: &str) -> Result<(), ApiError> {
    if value.trim().is_empty() {
        return Err(ApiError::unprocessable(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
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
        if !is_builtin_workspace_id(&mount.workspace_id) {
            require_ready_workspace(state, &mount.workspace_id)?;
        }
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
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct UpdateConnectionRequest {
    name: Option<String>,
    url: Option<String>,
    api_flavor: Option<ConnectionApiFlavor>,
    api_key: Option<String>,
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
    workspace_mounts: Option<Vec<WorkspaceMountRecord>>,
}

#[derive(Debug, Deserialize)]
struct UpdateGitAgentConfigRequest {
    enabled: Option<bool>,
    default_branch: Option<String>,
    allowed_ref_prefixes: Option<Vec<String>>,
    allowed_refs: Option<Vec<String>>,
    remote_url: Option<String>,
    patch_url: Option<String>,
    review_agent_id: Option<String>,
    validation_command: Option<String>,
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
    workspace_mounts: Vec<WorkspaceMountRecord>,
}

#[derive(Debug, Deserialize)]
struct SendMessageRequest {
    message: String,
}

#[derive(Debug, Deserialize)]
struct StartNewSessionRequest {
    session_id: String,
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

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    detail: String,
}

impl ApiError {
    const fn unauthorized(detail: String) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            detail,
        }
    }

    const fn not_found(detail: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            detail,
        }
    }

    const fn conflict(detail: String) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            detail,
        }
    }

    const fn unprocessable(detail: String) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            detail,
        }
    }

    const fn bad_gateway(detail: String) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            detail,
        }
    }

    const fn service_unavailable(detail: String) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            detail,
        }
    }

    const fn gateway_timeout(detail: String) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            detail,
        }
    }

    const fn payload_too_large(detail: String) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            detail,
        }
    }

    const fn internal(detail: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail,
        }
    }

    fn error_kind(&self) -> &'static str {
        match self.status {
            StatusCode::NOT_FOUND => "not_found",
            StatusCode::UNAUTHORIZED => "unauthorized",
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
        (status, Json(json!({ "detail": detail }))).into_response()
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

impl From<AgentHostError> for ApiError {
    fn from(error: AgentHostError) -> Self {
        match error {
            AgentHostError::HttpStatus { status, .. } if status == StatusCode::NOT_FOUND => {
                Self::not_found(format!("agent_host returned HTTP {status}"))
            }
            AgentHostError::HttpStatus { status, .. } if status.is_client_error() => Self {
                status,
                detail: format!("agent_host returned HTTP {status}"),
            },
            other => Self::bad_gateway(other.to_string()),
        }
    }
}

impl From<GitAgentError> for ApiError {
    fn from(error: GitAgentError) -> Self {
        match error {
            GitAgentError::HttpStatus { status, .. } if status == StatusCode::NOT_FOUND => {
                Self::not_found(format!("git_agent returned HTTP {status}"))
            }

            GitAgentError::HttpStatus { status, .. } if status.is_client_error() => Self {
                status,
                detail: format!("git_agent returned HTTP {status}"),
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
    use tokio::{
        net::TcpListener,
        sync::{Semaphore, mpsc},
        task::JoinHandle,
        time::sleep,
    };
    use tokio_stream::wrappers::ReceiverStream;
    use tower::ServiceExt;

    use super::send_stream_item;
    use crate::{
        ActiveTurnRecord, ActiveTurnStreamState, AppConfig, AppState, agent_host::AgentHostClient,
        build_router,
    };

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
        Ok(build_router(AppState::with_agent_host(config, agent_host)))
    }

    fn test_router_with_connection_models_timeout(
        timeout: Duration,
    ) -> Result<Router, Box<dyn Error + Send + Sync>> {
        let mut env = BTreeMap::new();
        env.insert("CLIENT_SERVICE_TEST".to_owned(), "enabled".to_owned());
        let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", env)
            .with_connection_models_timeout(timeout);
        let agent_host = AgentHostClient::new("http://127.0.0.1:9", Duration::from_millis(50))?;
        Ok(build_router(AppState::with_agent_host(config, agent_host)))
    }

    struct StreamingUpstream {
        base_url: String,
        handle: JoinHandle<Result<(), std::io::Error>>,
    }

    struct SessionControlUpstream {
        base_url: String,
        create_payload: Arc<Mutex<Option<Value>>>,
        handle: JoinHandle<Result<(), std::io::Error>>,
    }

    #[derive(Clone)]
    struct RestartingUpstreamState {
        create_payload: Arc<Mutex<Option<Value>>>,
        message_requests: Arc<Mutex<Vec<(String, String)>>>,
        reset_requests: Arc<Mutex<Vec<String>>>,
        old_started: Arc<Semaphore>,
        release_old: Arc<Semaphore>,
        fresh_started: Arc<Semaphore>,
        release_fresh: Arc<Semaphore>,
        reset_fails: bool,
        replay_fails: bool,
        old_termination: OldStreamTermination,
    }

    #[derive(Clone, Copy)]
    enum OldStreamTermination {
        Event,
        End,
        MalformedEvent,
    }

    struct RestartingUpstream {
        base_url: String,
        state: RestartingUpstreamState,
        handle: JoinHandle<Result<(), std::io::Error>>,
    }

    impl RestartingUpstream {
        async fn start(
            reset_fails: bool,
            replay_fails: bool,
        ) -> Result<Self, Box<dyn Error + Send + Sync>> {
            Self::start_with_termination(reset_fails, replay_fails, OldStreamTermination::Event)
                .await
        }

        async fn start_with_termination(
            reset_fails: bool,
            replay_fails: bool,
            old_termination: OldStreamTermination,
        ) -> Result<Self, Box<dyn Error + Send + Sync>> {
            let state = RestartingUpstreamState {
                create_payload: Arc::new(Mutex::new(None)),
                message_requests: Arc::new(Mutex::new(Vec::new())),
                reset_requests: Arc::new(Mutex::new(Vec::new())),
                old_started: Arc::new(Semaphore::new(0)),
                release_old: Arc::new(Semaphore::new(0)),
                fresh_started: Arc::new(Semaphore::new(0)),
                release_fresh: Arc::new(Semaphore::new(0)),
                reset_fails,
                replay_fails,
                old_termination,
            };
            let app = Router::new()
                .route("/sessions", post(restarting_create_session))
                .route("/sessions/{session_id}", get(upstream_get_session))
                .route(
                    "/sessions/{session_id}/reset",
                    post(restarting_reset_session),
                )
                .route(
                    "/sessions/{session_id}/messages/stream",
                    post(restarting_stream_message),
                )
                .with_state(state.clone());
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let handle = tokio::spawn(axum::serve(listener, app).into_future());
            Ok(Self {
                base_url: format_base_url(address),
                state,
                handle,
            })
        }
    }

    impl Drop for RestartingUpstream {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    impl SessionControlUpstream {
        async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
            let create_payload = Arc::new(Mutex::new(None));
            let app = Router::new()
                .route("/sessions", post(capture_upstream_create_session))
                .with_state(create_payload.clone());
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let handle = tokio::spawn(axum::serve(listener, app).into_future());

            Ok(Self {
                base_url: format_base_url(address),
                create_payload,
                handle,
            })
        }
    }

    impl Drop for SessionControlUpstream {
        fn drop(&mut self) {
            self.handle.abort();
        }
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

    async fn capture_upstream_create_session(
        State(create_payload): State<Arc<Mutex<Option<Value>>>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        if let Ok(mut payload) = create_payload.lock() {
            *payload = Some(body);
        }
        Json(json!({ "session_id": "upstream-session", "status": "idle" }))
    }

    async fn restarting_create_session(
        State(state): State<RestartingUpstreamState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        if let Ok(mut payload) = state.create_payload.lock() {
            *payload = Some(body);
        }
        Json(json!({ "session_id": "old-session", "status": "idle" }))
    }

    async fn restarting_reset_session(
        State(state): State<RestartingUpstreamState>,
        Path(session_id): Path<String>,
    ) -> Response {
        if let Ok(mut requests) = state.reset_requests.lock() {
            requests.push(session_id);
        }
        if state.reset_fails {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": "reset failed" })),
            )
                .into_response();
        }
        (
            StatusCode::OK,
            Json(json!({ "session_id": "fresh-session", "status": "idle" })),
        )
            .into_response()
    }

    async fn restarting_stream_message(
        State(state): State<RestartingUpstreamState>,
        Path(session_id): Path<String>,
        Json(body): Json<Value>,
    ) -> Response {
        let message = body["message"].as_str().unwrap_or_default().to_owned();
        if let Ok(mut requests) = state.message_requests.lock() {
            requests.push((session_id.clone(), message));
        }
        if session_id == "fresh-session" && state.replay_fails {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": "replay failed" })),
            )
                .into_response();
        }

        let (sender, receiver) = mpsc::channel::<Result<Vec<u8>, Infallible>>(4);
        tokio::spawn(async move {
            if session_id == "old-session" {
                let _sent = sender
                    .send(Ok(test_ndjson_line(
                        &json!({ "type": "text_delta", "content": "old-before" }),
                    )))
                    .await;
                state.old_started.add_permits(1);
                if state.release_old.acquire().await.is_err() {
                    return;
                }
                match state.old_termination {
                    OldStreamTermination::Event => {
                        let _sent = sender
                            .send(Ok(test_ndjson_line(
                                &json!({ "type": "text_delta", "content": "old-after" }),
                            )))
                            .await;
                    }
                    OldStreamTermination::End => {}
                    OldStreamTermination::MalformedEvent => {
                        let _sent = sender.send(Ok(Vec::from("{invalid-json}\n"))).await;
                    }
                }
            } else {
                state.fresh_started.add_permits(1);
                if state.release_fresh.acquire().await.is_err() {
                    return;
                }
                let _sent = sender
                    .send(Ok(test_ndjson_line(
                        &json!({ "type": "text_delta", "content": "fresh-answer" }),
                    )))
                    .await;
            }
        });
        (
            StatusCode::OK,
            Body::from_stream(ReceiverStream::new(receiver)),
        )
            .into_response()
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

    async fn request_json_with_authorization(
        app: Router,
        path: &str,
        body: Value,
        authorization: Option<&str>,
    ) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(authorization) = authorization {
            builder = builder.header(header::AUTHORIZATION, authorization);
        }
        let response = app
            .oneshot(builder.body(Body::from(serde_json::to_vec(&body)?))?)
            .await?;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        let value = serde_json::from_slice(&body)
            .unwrap_or_else(|_error| Value::String(String::from_utf8_lossy(&body).into_owned()));
        Ok((status, value))
    }

    async fn restart_test_app(
        upstream: &RestartingUpstream,
    ) -> Result<(AppState, Router, String, String), Box<dyn Error + Send + Sync>> {
        let config = AppConfig::new("127.0.0.1", 0, &upstream.base_url, BTreeMap::new());
        let agent_host = AgentHostClient::new(&upstream.base_url, Duration::from_secs(1))?;
        let state = AppState::with_agent_host(config, agent_host);
        let app = build_router(state.clone());
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
        let session_id = session["session_id"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("session id missing"))?
            .to_owned();
        let create_payload = upstream
            .state
            .create_payload
            .lock()
            .map_err(|_error| std::io::Error::other("create payload lock poisoned"))?
            .clone()
            .ok_or_else(|| std::io::Error::other("create payload missing"))?;
        let token = create_payload["env"]["AGENTSPACE_SESSION_CONTROL_TOKEN"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("control token missing"))?
            .to_owned();
        Ok((state, app, session_id, token))
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
    async fn session_control_identity_and_endpoint_are_private_authenticated_and_idempotent()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = SessionControlUpstream::start().await?;
        let config = AppConfig::new("127.0.0.1", 0, &upstream.base_url, BTreeMap::new());
        let agent_host = AgentHostClient::new(&upstream.base_url, Duration::from_secs(1))?;
        let state = AppState::with_agent_host(config, agent_host);
        let app = build_router(state.clone());

        let (status, _agent) = request_json(
            app.clone(),
            Method::POST,
            "/agents",
            Some(json!({ "agent_id": "agent-one", "name": "Agent One" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let (status, session_summary) = request_json(
            app.clone(),
            Method::POST,
            "/sessions",
            Some(json!({ "agent_id": "agent-one" })),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let session_id = session_summary["session_id"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("session_id missing"))?
            .to_owned();
        let create_payload = upstream
            .create_payload
            .lock()
            .map_err(|_error| std::io::Error::other("create payload lock poisoned"))?
            .clone()
            .ok_or_else(|| std::io::Error::other("upstream create payload missing"))?;
        let env = create_payload["env"]
            .as_object()
            .ok_or_else(|| std::io::Error::other("upstream environment missing"))?;
        let token = env["AGENTSPACE_SESSION_CONTROL_TOKEN"]
            .as_str()
            .ok_or_else(|| std::io::Error::other("session control token missing"))?
            .to_owned();
        assert_eq!(env["AGENTSPACE_SESSION_ID"], session_id);
        assert_eq!(token.len(), 64);

        let stored = state
            .sessions
            .get(&session_id)?
            .ok_or_else(|| std::io::Error::other("stored session missing"))?;
        let stored_hash = stored
            .session_control_token_hash
            .as_deref()
            .ok_or_else(|| std::io::Error::other("stored capability hash missing"))?;
        assert_eq!(stored_hash, super::hash_session_control_token(&token));
        let public_summary = serde_json::to_string(&session_summary)?;
        assert!(!public_summary.contains(&token));
        assert!(!public_summary.contains(stored_hash));

        let endpoint = "/internal/session-control/start-new";
        let payload = json!({ "session_id": session_id });
        let expected_auth_error = json!({ "detail": "invalid session control credentials" });
        for (authorization, request_payload) in [
            (None, payload.clone()),
            (Some("Basic invalid".to_owned()), payload.clone()),
            (Some("Bearer wrong-token".to_owned()), payload.clone()),
            (
                Some(format!("Bearer {token}")),
                json!({ "session_id": "missing-session" }),
            ),
        ] {
            let (status, response) = request_json_with_authorization(
                app.clone(),
                endpoint,
                request_payload,
                authorization.as_deref(),
            )
            .await?;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(response, expected_auth_error);
        }

        let authorization = format!("Bearer {token}");
        let (status, _response) =
            request_json_with_authorization(app.clone(), endpoint, json!({}), Some(&authorization))
                .await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

        let (status, response) = request_json_with_authorization(
            app.clone(),
            endpoint,
            payload.clone(),
            Some(&authorization),
        )
        .await?;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            response,
            json!({ "detail": "session does not have an active turn" })
        );

        state
            .active_turns
            .lock()
            .map_err(|_error| std::io::Error::other("active turn lock poisoned"))?
            .insert(
                session_id.clone(),
                ActiveTurnRecord {
                    turn_id: "turn-one".to_owned(),
                    user_message_id: "user-one".to_owned(),
                    assistant_message_id: "assistant-one".to_owned(),
                    start_new_requested: false,
                    automatic_restart_count: 0,
                    restart_requests_closed: false,
                    stream: None,
                },
            );
        for _request_number in 0..2 {
            let (status, response) = request_json_with_authorization(
                app.clone(),
                endpoint,
                payload.clone(),
                Some(&authorization),
            )
            .await?;
            assert_eq!(status, StatusCode::ACCEPTED);
            assert_eq!(response, json!({ "accepted": true, "turn_id": "turn-one" }));
        }
        let active_turns = state
            .active_turns
            .try_lock()
            .map_err(|_error| std::io::Error::other("active turn mutex remained locked"))?;
        assert!(active_turns[&session_id].start_new_requested);
        drop(active_turns);

        Ok(())
    }

    #[tokio::test]
    async fn streaming_turn_restarts_once_and_keeps_original_subscription()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = RestartingUpstream::start(false, false).await?;
        let (state, app, session_id, token) = restart_test_app(&upstream).await?;
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/sessions/{session_id}/messages/stream"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "message": "new topic" }),
            )?))?;
        let response = app.clone().oneshot(request).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let first = body
            .frame()
            .await
            .ok_or_else(|| std::io::Error::other("old event missing"))??
            .into_data()
            .map_err(|_frame| std::io::Error::other("old event was not data"))?;
        let first = serde_json::from_slice::<Value>(&first)?;
        assert_eq!(first["event"]["content"], "old-before");

        let authorization = format!("Bearer {token}");
        let (status, accepted) = request_json_with_authorization(
            app.clone(),
            "/internal/session-control/start-new",
            json!({ "session_id": session_id }),
            Some(&authorization),
        )
        .await?;
        assert_eq!(status, StatusCode::ACCEPTED);
        let original_turn_id = accepted["turn_id"].clone();
        upstream.state.release_old.add_permits(1);

        let restart = tokio::time::timeout(Duration::from_secs(1), body.frame())
            .await?
            .ok_or_else(|| std::io::Error::other("restart event missing"))??
            .into_data()
            .map_err(|_frame| std::io::Error::other("restart event was not data"))?;
        let restart = serde_json::from_slice::<Value>(&restart)?;
        assert_eq!(restart["event"]["type"], "agentspace/session-restarted");
        upstream.state.fresh_started.acquire().await?.forget();

        let (status, repeated) = request_json_with_authorization(
            app.clone(),
            "/internal/session-control/start-new",
            json!({ "session_id": session_id }),
            Some(&authorization),
        )
        .await?;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            repeated["detail"],
            "fresh-session handoff already occurred for this turn"
        );
        upstream.state.release_fresh.add_permits(1);

        let rest = tokio::time::timeout(Duration::from_secs(1), body.collect())
            .await??
            .to_bytes();
        let chunks = std::str::from_utf8(&rest)?
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["event"]["content"], "fresh-answer");
        let final_payload = &chunks[1];
        assert_eq!(final_payload["type"], "final");
        assert_eq!(final_payload["turn_id"], original_turn_id);
        assert_eq!(final_payload["completed"], true);
        assert_eq!(final_payload["automatic_restart_count"], 1);
        assert_eq!(
            final_payload["assistant_message"]["content"],
            "fresh-answer"
        );
        assert_eq!(final_payload["session"]["session_id"], session_id);
        assert_eq!(
            final_payload["session"]["agent_host_session_id"],
            "fresh-session"
        );

        let stored = state
            .sessions
            .get(&session_id)?
            .ok_or_else(|| std::io::Error::other("stored session missing"))?;
        assert_eq!(stored.agent_host_session_id, "fresh-session");
        assert_eq!(stored.messages.len(), 2);
        assert_eq!(stored.messages[0].content, "new topic");
        assert_eq!(stored.messages[1].content, "fresh-answer");
        let message_requests = upstream
            .state
            .message_requests
            .lock()
            .map_err(|_error| std::io::Error::other("message request lock poisoned"))?
            .clone();
        assert_eq!(
            message_requests,
            vec![
                ("old-session".to_owned(), "new topic".to_owned()),
                ("fresh-session".to_owned(), "new topic".to_owned()),
            ]
        );
        let reset_requests = upstream
            .state
            .reset_requests
            .lock()
            .map_err(|_error| std::io::Error::other("reset request lock poisoned"))?
            .clone();
        assert_eq!(reset_requests, vec!["old-session"]);

        Ok(())
    }

    #[tokio::test]
    async fn synchronous_turn_restarts_and_returns_only_fresh_answer()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let upstream = RestartingUpstream::start(false, false).await?;
        let (state, app, session_id, token) = restart_test_app(&upstream).await?;
        let request_app = app.clone();
        let request_session_id = session_id.clone();
        let response_task = tokio::spawn(async move {
            request_json(
                request_app,
                Method::POST,
                &format!("/sessions/{request_session_id}/messages"),
                Some(json!({ "message": "new topic" })),
            )
            .await
        });
        upstream.state.old_started.acquire().await?.forget();

        let authorization = format!("Bearer {token}");
        let (status, accepted) = request_json_with_authorization(
            app.clone(),
            "/internal/session-control/start-new",
            json!({ "session_id": session_id }),
            Some(&authorization),
        )
        .await?;
        assert_eq!(status, StatusCode::ACCEPTED);
        upstream.state.release_old.add_permits(1);
        upstream.state.fresh_started.acquire().await?.forget();

        let (status, repeated) = request_json_with_authorization(
            app,
            "/internal/session-control/start-new",
            json!({ "session_id": session_id }),
            Some(&authorization),
        )
        .await?;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            repeated["detail"],
            "fresh-session handoff already occurred for this turn"
        );
        upstream.state.release_fresh.add_permits(1);
        let (status, response) = response_task.await??;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["turn_id"], accepted["turn_id"]);
        assert_eq!(response["completed"], true);
        assert_eq!(response["automatic_restart_count"], 1);
        assert_eq!(response["assistant_message"]["content"], "fresh-answer");
        assert_eq!(response["session"]["session_id"], session_id);
        assert_eq!(
            response["session"]["agent_host_session_id"],
            "fresh-session"
        );
        let stored = state
            .sessions
            .get(&session_id)?
            .ok_or_else(|| std::io::Error::other("stored session missing"))?;
        assert_eq!(stored.messages.len(), 2);
        assert_eq!(stored.messages[1].content, "fresh-answer");

        Ok(())
    }

    #[tokio::test]
    async fn upstream_end_or_error_after_request_still_replays()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        for termination in [
            OldStreamTermination::End,
            OldStreamTermination::MalformedEvent,
        ] {
            let upstream =
                RestartingUpstream::start_with_termination(false, false, termination).await?;
            let (_state, app, session_id, token) = restart_test_app(&upstream).await?;
            let request_app = app.clone();
            let request_session_id = session_id.clone();
            let response_task = tokio::spawn(async move {
                request_json(
                    request_app,
                    Method::POST,
                    &format!("/sessions/{request_session_id}/messages"),
                    Some(json!({ "message": "new topic" })),
                )
                .await
            });
            upstream.state.old_started.acquire().await?.forget();
            let authorization = format!("Bearer {token}");
            let (status, _accepted) = request_json_with_authorization(
                app,
                "/internal/session-control/start-new",
                json!({ "session_id": session_id }),
                Some(&authorization),
            )
            .await?;
            assert_eq!(status, StatusCode::ACCEPTED);
            upstream.state.release_old.add_permits(1);
            upstream.state.fresh_started.acquire().await?.forget();
            upstream.state.release_fresh.add_permits(1);

            let (status, response) = response_task.await??;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(response["completed"], true);
            assert_eq!(response["automatic_restart_count"], 1);
            assert_eq!(response["assistant_message"]["content"], "fresh-answer");
        }

        Ok(())
    }

    #[tokio::test]
    async fn restart_and_replay_failures_are_terminal_without_old_context_fallback()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        for termination in [
            OldStreamTermination::Event,
            OldStreamTermination::End,
            OldStreamTermination::MalformedEvent,
        ] {
            for (reset_fails, replay_fails) in [(true, false), (false, true)] {
                let upstream = RestartingUpstream::start_with_termination(
                    reset_fails,
                    replay_fails,
                    termination,
                )
                .await?;
                let (state, app, session_id, token) = restart_test_app(&upstream).await?;
                let request_app = app.clone();
                let request_session_id = session_id.clone();
                let response_task = tokio::spawn(async move {
                    request_json(
                        request_app,
                        Method::POST,
                        &format!("/sessions/{request_session_id}/messages"),
                        Some(json!({ "message": "new topic" })),
                    )
                    .await
                });
                upstream.state.old_started.acquire().await?.forget();
                let authorization = format!("Bearer {token}");
                let (status, _accepted) = request_json_with_authorization(
                    app,
                    "/internal/session-control/start-new",
                    json!({ "session_id": session_id }),
                    Some(&authorization),
                )
                .await?;
                assert_eq!(status, StatusCode::ACCEPTED);
                upstream.state.release_old.add_permits(1);

                let (status, response) = response_task.await??;
                assert_eq!(status, StatusCode::OK);
                assert_eq!(response["completed"], false);
                assert_eq!(response["automatic_restart_count"], 1);
                assert!(
                    response["error"]
                        .as_str()
                        .is_some_and(|error| !error.is_empty())
                );
                assert_eq!(response["assistant_message"]["content"], "");
                let stored = state
                    .sessions
                    .get(&session_id)?
                    .ok_or_else(|| std::io::Error::other("stored session missing"))?;
                assert_eq!(stored.status, "error");
                assert_eq!(stored.messages.len(), 2);
                assert_eq!(stored.messages[0].content, "new topic");
                assert_eq!(stored.messages[1].content, "");
            }
        }

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
                "skills": ["skill-a"],
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
