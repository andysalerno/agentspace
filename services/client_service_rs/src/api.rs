use std::{collections::BTreeMap, convert::Infallible, str::FromStr};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::{
    AppState, ENV_PREFIX,
    agent_host::{AgentHostError, JsonObject, KernelEvent},
    errors::{StoreError, ValidationError},
    models::{
        AgentRecord, ClientType, ConnectionApiFlavor, ConnectionRecord, GatewayRecord, GatewayType,
        HarnessName, MessageRecord, MessageRole, SessionRecord, ToolCallRecord, parse_env_vars,
        utc_now, validate_agent_id, validate_connection_id, validate_gateway_id, validate_skill_id,
    },
};

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

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn info(State(state): State<AppState>) -> Json<Value> {
    let agent_host = match state.agent_host.info().await {
        Ok(info) => Value::Object(info),
        Err(error) => json!({ "service": "agent_host", "error": error.to_string() }),
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
    Json(
        HarnessName::all()
            .iter()
            .map(|harness| harness.as_str())
            .collect(),
    )
}

async fn list_kernel_configs(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let configs = state
        .kernel_configs
        .list()?
        .into_iter()
        .map(|record| record.summary())
        .collect();
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
    Ok(Json(value))
}

async fn update_kernel_config(
    State(state): State<AppState>,
    Path(raw_harness): Path<String>,
    Json(payload): Json<UpdateKernelConfigRequest>,
) -> Result<Json<Value>, ApiError> {
    let harness = parse_harness(&raw_harness)?;
    let record = state.kernel_configs.upsert(harness, payload.env_vars)?;
    Ok(Json(record.summary()))
}

async fn list_connections(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let connections = state
        .connections
        .list()?
        .into_iter()
        .map(|connection| connection.summary(false))
        .collect();
    Ok(Json(connections))
}

async fn get_connection(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let connection = require_connection(&state, &connection_id)?;
    Ok(Json(connection.summary(false)))
}

async fn list_connection_models(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let connection = require_connection(&state, &connection_id)?;
    let url = format!("{}/models", connection.url.trim_end_matches('/'));
    let mut request = state.http_client.get(url);
    if !connection.api_key.is_empty() {
        request = request.bearer_auth(&connection.api_key);
    }
    let response = request.send().await.map_err(|error| {
        ApiError::bad_gateway(format!(
            "failed to fetch models for connection {connection_id}: {error}"
        ))
    })?;
    let response = if response.status().is_success() {
        response
    } else {
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
        return Err(ApiError::bad_gateway(format!(
            "models response for connection {connection_id} was not a JSON object"
        )));
    }
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
    Ok(Json(value))
}

async fn delete_connection(
    State(state): State<AppState>,
    Path(connection_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.connections.delete(&connection_id)? {
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
    let value = agent.summary();
    state.agents.insert(agent)?;
    Ok(Json(value))
}

async fn list_agents(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let agents = state
        .agents
        .list()?
        .into_iter()
        .map(|agent| agent.summary())
        .collect();
    Ok(Json(agents))
}

async fn get_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let agent = require_agent(&state, &agent_id)?;
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
        let _removed = state.sessions.delete(&session.session_id)?;
        state
            .agent_host
            .destroy_session(&session.agent_host_session_id)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<Value>, ApiError> {
    let agent = require_agent(&state, &payload.agent_id)?;
    let env = session_env(&state, &agent)?;
    let upstream = state
        .agent_host
        .create_session(agent.harness.as_str(), Some(&agent.skills), Some(&env))
        .await?;
    let upstream_session_id = string_field(&upstream, "session_id")?;
    let status = string_field(&upstream, "status")?;
    let session = SessionRecord::new(
        Uuid::now_v7().simple().to_string(),
        payload.agent_id,
        upstream_session_id,
        status,
        payload.channel_name,
        payload.client_type,
    );
    let value = session.summary();
    state.sessions.insert(session)?;
    Ok(Json(value))
}

async fn list_sessions(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let sessions = state
        .sessions
        .list()?
        .into_iter()
        .map(|session| session.summary())
        .collect();
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
    Ok(Json(session.detail()))
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
    Ok(Json(json!({ "messages": messages })))
}

async fn send_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(run_turn(&state, &session_id, &payload.message).await?))
}

async fn stream_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<Response, ApiError> {
    let turn = start_streaming_turn(&state, &session_id, payload.message)?;
    let (sender, receiver) = mpsc::channel(16);
    tokio::spawn(run_streaming_turn(state, turn, sender));
    Ok(ndjson_stream_response(receiver))
}

async fn stream_turn(
    State(state): State<AppState>,
    Path((session_id, turn_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let _session = require_session(&state, &session_id)?;
    Err(ApiError::not_found(format!(
        "turn not found: {turn_id}; active turn replay is not implemented in client_service_rs yet"
    )))
}

async fn reset_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut session = require_session(&state, &session_id)?;
    let upstream = state
        .agent_host
        .reset_session(&session.agent_host_session_id)
        .await?;
    session.agent_host_session_id = string_field(&upstream, "session_id")?;
    session.status = string_field(&upstream, "status")?;
    session.updated_at = utc_now();
    state.sessions.clear_messages(&session_id)?;
    state.sessions.update(session.clone())?;
    Ok(Json(session.summary()))
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
    Ok(StatusCode::NO_CONTENT)
}

async fn kernel_logs(
    State(state): State<AppState>,
    Path(kernel_session_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_kernel(&state, &kernel_session_id).await?;
    let lines = state.agent_host.logs(&kernel_session_id).await?;
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
    Ok(Json(json!({ "lines": lines })))
}

async fn create_skill(
    State(state): State<AppState>,
    Json(payload): Json<CreateSkillRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_skill_id(&payload.skill_id)?;
    let skill = state
        .agent_host
        .create_skill(&payload.skill_id, &payload.files)
        .await?;
    Ok(Json(Value::Object(skill)))
}

async fn list_skills(State(state): State<AppState>) -> Result<Json<Vec<Value>>, ApiError> {
    let skills = state
        .agent_host
        .list_skills()
        .await?
        .into_iter()
        .map(Value::Object)
        .collect();
    Ok(Json(skills))
}

async fn get_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let skill = state.agent_host.get_skill(&skill_id).await?;
    Ok(Json(Value::Object(skill)))
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
    Ok(Json(Value::Object(skill)))
}

async fn delete_skill(
    State(state): State<AppState>,
    Path(skill_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.agent_host.delete_skill(&skill_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_gateway_types() -> Json<Vec<&'static str>> {
    Json(
        GatewayType::all()
            .iter()
            .map(|gateway_type| gateway_type.as_str())
            .collect(),
    )
}

async fn get_gateway_type_schema(
    Path(raw_gateway_type): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let gateway_type = parse_gateway_type(&raw_gateway_type)?;
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
        .collect();
    Ok(Json(gateways))
}

async fn create_gateway(
    State(state): State<AppState>,
    Json(payload): Json<CreateGatewayRequest>,
) -> Result<Json<Value>, ApiError> {
    validate_gateway_id(&payload.gateway_id)?;
    require_agent(&state, &payload.agent_id)?;
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
    if payload.enabled {
        return start_gateway_by_id(&state, &gateway_id).await.map(Json);
    }
    let gateway = require_gateway(&state, &gateway_id)?;
    Ok(Json(gateway.summary(false)))
}

async fn get_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let gateway = require_gateway(&state, &gateway_id)?;
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
    state.gateways.update(gateway)?;
    if enabled && !previously_enabled {
        start_gateway_by_id(&state, &gateway_id).await.map(Json)
    } else if !enabled && previously_enabled {
        stop_gateway_by_id(&state, &gateway_id).await.map(Json)
    } else if config_changed && was_running {
        let _stopped = stop_gateway_by_id(&state, &gateway_id).await?;
        start_gateway_by_id(&state, &gateway_id).await.map(Json)
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
        let _ignored = state.agent_host.destroy_gateway(&gateway_id).await;
    }
    let _removed = state.gateways.delete(&gateway_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    start_gateway_by_id(&state, &gateway_id).await.map(Json)
}

async fn stop_gateway(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    stop_gateway_by_id(&state, &gateway_id).await.map(Json)
}

async fn gateway_logs(
    State(state): State<AppState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _gateway = require_gateway(&state, &gateway_id)?;
    let lines = state.agent_host.gateway_logs(&gateway_id).await?;
    Ok(Json(json!({ "lines": lines })))
}

async fn start_gateway_by_id(state: &AppState, gateway_id: &str) -> Result<Value, ApiError> {
    let mut gateway = require_gateway(state, gateway_id)?;
    "starting".clone_into(&mut gateway.status);
    gateway.last_error = None;
    gateway.updated_at = utc_now();
    state.gateways.update(gateway.clone())?;
    let env = gateway.effective_env();
    match state
        .agent_host
        .create_gateway(
            gateway_id,
            gateway.gateway_type.as_str(),
            &gateway.agent_id,
            &env,
        )
        .await
    {
        Ok(response) => {
            "running".clone_into(&mut gateway.status);
            gateway.container_name = response
                .get("container_name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            gateway.updated_at = utc_now();
            state.gateways.update(gateway.clone())?;
            Ok(gateway.summary(false))
        }
        Err(error) => {
            "error".clone_into(&mut gateway.status);
            gateway.last_error = Some(error.to_string());
            gateway.updated_at = utc_now();
            state.gateways.update(gateway)?;
            Err(error.into())
        }
    }
}

async fn stop_gateway_by_id(state: &AppState, gateway_id: &str) -> Result<Value, ApiError> {
    let mut gateway = require_gateway(state, gateway_id)?;
    if let Err(error) = state.agent_host.destroy_gateway(gateway_id).await {
        gateway.last_error = Some(error.to_string());
    }
    "stopped".clone_into(&mut gateway.status);
    gateway.container_name = None;
    gateway.updated_at = utc_now();
    state.gateways.update(gateway.clone())?;
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
        Ok(())
    } else {
        Err(ApiError::not_found(format!(
            "kernel {kernel_session_id:?} not found"
        )))
    }
}

fn session_env(
    state: &AppState,
    agent: &AgentRecord,
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
    if !agent.system_prompt.is_empty() {
        env.insert(
            "KERNEL_SYSTEM_PROMPT".to_owned(),
            agent.system_prompt.clone(),
        );
    }
    Ok(env)
}

struct StreamingTurn {
    turn_id: String,
    session_id: String,
    agent_host_session_id: String,
    message: String,
    assistant_message_id: String,
    assistant_created_at: String,
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
                if active_turns.get(&self.session_id) == Some(&self.turn_id) {
                    active_turns.remove(&self.session_id);
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

type NdjsonSender = mpsc::Sender<Result<Vec<u8>, Infallible>>;
type NdjsonReceiver = mpsc::Receiver<Result<Vec<u8>, Infallible>>;

fn start_streaming_turn(
    state: &AppState,
    session_id: &str,
    message: String,
) -> Result<StreamingTurn, ApiError> {
    let mut session = require_session(state, session_id)?;
    let turn_id = Uuid::now_v7().simple().to_string();
    let active_turn = begin_active_turn(state, &session.session_id, &turn_id)?;
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

    "busy".clone_into(&mut session.status);
    session.updated_at = utc_now();
    state.sessions.update(session.clone())?;
    state.sessions.append_message(user_message)?;
    state.sessions.append_message(assistant_message)?;

    Ok(StreamingTurn {
        turn_id,
        session_id: session.session_id,
        agent_host_session_id: session.agent_host_session_id,
        message,
        assistant_message_id,
        assistant_created_at,
        _active_turn: active_turn,
    })
}

fn begin_active_turn(
    state: &AppState,
    session_id: &str,
    turn_id: &str,
) -> Result<ActiveTurnGuard, ApiError> {
    let mut active_turns = state
        .active_turns
        .lock()
        .map_err(|_error| ApiError::internal("active turn lock poisoned".to_owned()))?;
    if let Some(existing_turn_id) = active_turns.get(session_id) {
        return Err(ApiError::conflict(format!(
            "session {session_id:?} already has active turn {existing_turn_id:?}"
        )));
    }
    active_turns.insert(session_id.to_owned(), turn_id.to_owned());
    drop(active_turns);
    Ok(ActiveTurnGuard {
        state: state.clone(),
        session_id: session_id.to_owned(),
        turn_id: turn_id.to_owned(),
    })
}

async fn run_streaming_turn(state: AppState, turn: StreamingTurn, sender: NdjsonSender) {
    let mut events = Vec::new();
    let mut completed = false;
    let mut error = None;

    match state
        .agent_host
        .stream_message(&turn.agent_host_session_id, &turn.message)
        .await
    {
        Ok(mut stream) => loop {
            match stream.next_event().await {
                Ok(Some(event)) => {
                    events.push(event);
                    let assistant_message = assistant_message_from_events(
                        &turn.session_id,
                        &turn.assistant_message_id,
                        &turn.assistant_created_at,
                        &events,
                    );
                    if let Err(store_error) = state.sessions.update_message(assistant_message) {
                        error = Some(store_error.to_string());
                        break;
                    }

                    let event = Value::Object(events.last().cloned().unwrap_or_default());
                    let _sent = send_ndjson_item(
                        &sender,
                        &json!({
                            "type": "event",
                            "event": event,
                        }),
                    )
                    .await;
                }
                Ok(None) => {
                    completed = true;
                    break;
                }
                Err(stream_error) => {
                    error = Some(stream_error.to_string());
                    break;
                }
            }
        },
        Err(stream_error) => {
            error = Some(stream_error.to_string());
        }
    }

    let final_payload =
        match finalize_streaming_turn(&state, &turn, &events, completed, error.as_deref()).await {
            Ok(payload) => payload,
            Err(finalize_error) => json!({
                "type": "final",
                "turn_id": turn.turn_id,
                "completed": false,
                "error": finalize_error.detail,
            }),
        };
    let _sent = send_ndjson_item(&sender, &final_payload).await;
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
    state.sessions.update_message(assistant_message.clone())?;

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

async fn send_ndjson_item(sender: &NdjsonSender, value: &Value) -> bool {
    match ndjson_line_bytes(value) {
        Ok(line) => sender.send(Ok(line)).await.is_ok(),
        Err(_error) => false,
    }
}

async fn run_turn(state: &AppState, session_id: &str, message: &str) -> Result<Value, ApiError> {
    let mut session = require_session(state, session_id)?;
    let turn_id = Uuid::now_v7().simple().to_string();
    let _active_turn = begin_active_turn(state, &session.session_id, &turn_id)?;
    let user_message = MessageRecord::new(
        Uuid::now_v7().simple().to_string(),
        session.session_id.clone(),
        MessageRole::User,
        message,
    );
    let assistant_message_id = Uuid::now_v7().simple().to_string();
    let assistant_created_at = utc_now();
    "busy".clone_into(&mut session.status);
    session.updated_at = utc_now();
    state.sessions.update(session.clone())?;
    state.sessions.append_message(user_message)?;

    let events = state
        .agent_host
        .send_message(&session.agent_host_session_id, message)
        .await?;
    let assistant_message = assistant_message_from_events(
        &session.session_id,
        &assistant_message_id,
        &assistant_created_at,
        &events,
    );
    state.sessions.append_message(assistant_message.clone())?;
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
    Ok(json!({
        "session": session.summary(),
        "assistant_message": assistant_message.summary(),
        "events": events,
        "turn_id": turn_id,
        "completed": true,
    }))
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
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        Body::from_stream(ReceiverStream::new(receiver)),
    )
        .into_response()
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

fn require_session(state: &AppState, session_id: &str) -> Result<SessionRecord, ApiError> {
    state
        .sessions
        .get(session_id)?
        .ok_or_else(|| ApiError::not_found(format!("session {session_id:?} not found")))
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
    #[serde(default)]
    system_prompt: String,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    env_vars: String,
    connection_id: Option<String>,
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
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    agent_id: String,
    channel_name: Option<String>,
    client_type: Option<ClientType>,
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

    const fn internal(detail: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            detail,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "detail": self.detail }))).into_response()
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
            | StoreError::SessionAlreadyExists { .. } => Self::conflict(error.to_string()),
            StoreError::AgentNotFound { .. }
            | StoreError::ConnectionNotFound { .. }
            | StoreError::GatewayNotFound { .. }
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
            AgentHostError::HttpStatus { status, body } if status == StatusCode::NOT_FOUND => {
                Self::not_found(body)
            }
            AgentHostError::HttpStatus { status, body } if status.is_client_error() => Self {
                status,
                detail: body,
            },
            other => Self::bad_gateway(other.to_string()),
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
        time::{Duration, Instant},
    };

    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        extract::{Path, State},
        http::{Method, Request, StatusCode, header},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::sleep};
    use tokio_stream::wrappers::ReceiverStream;
    use tower::ServiceExt;

    use crate::{AppConfig, AppState, agent_host::AgentHostClient, build_router};

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

    struct StreamingUpstream {
        base_url: String,
        handle: JoinHandle<Result<(), std::io::Error>>,
    }

    impl StreamingUpstream {
        async fn start(final_delay: Duration) -> Result<Self, Box<dyn Error + Send + Sync>> {
            let app = Router::new()
                .route("/sessions", post(upstream_create_session))
                .route("/sessions/{session_id}", get(upstream_get_session))
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
        assert_eq!(value["system_prompt"], "");
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
            Some(&header::HeaderValue::from_static("application/x-ndjson"))
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static("no-cache"))
        );
        assert_eq!(
            response
                .headers()
                .get(header::HeaderName::from_static("x-accel-buffering")),
            Some(&header::HeaderValue::from_static("no"))
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
