#![allow(clippy::similar_names, clippy::too_many_lines)]

use std::{
    collections::BTreeMap,
    error::Error,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Query, State},
    http::{Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use client_service_rs::{
    AppConfig, AppState, agent_host::AgentHostClient, api::start_enabled_gateways, build_router,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecordedRequest {
    method: Method,
    path: String,
    query: Option<String>,
    body: Option<Value>,
}

#[derive(Clone, Default)]
struct StubState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl StubState {
    fn record(
        &self,
        method: Method,
        path: impl Into<String>,
        query: Option<String>,
        body: Option<Value>,
    ) -> Result<(), StatusCode> {
        self.requests
            .lock()
            .map_err(|_error| StatusCode::INTERNAL_SERVER_ERROR)?
            .push(RecordedRequest {
                method,
                path: path.into(),
                query,
                body,
            });
        Ok(())
    }

    fn recorded(&self) -> Result<Vec<RecordedRequest>, Box<dyn Error + Send + Sync>> {
        let requests = self
            .requests
            .lock()
            .map_err(|_error| "stub request mutex poisoned")?;
        Ok(requests.clone())
    }

    fn clear_recorded(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.requests
            .lock()
            .map_err(|_error| "stub request mutex poisoned")?
            .clear();
        Ok(())
    }
}

struct TestServer {
    base_url: String,
    state: StubState,
    handle: JoinHandle<Result<(), std::io::Error>>,
}

impl TestServer {
    async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let state = StubState::default();
        let app = stub_router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let handle = tokio::spawn(async move { axum::serve(listener, app).await });
        Ok(Self {
            base_url: format_base_url(address),
            state,
            handle,
        })
    }

    fn app(&self) -> Result<Router, Box<dyn Error + Send + Sync>> {
        Ok(build_router(self.app_state()?))
    }

    fn app_state(&self) -> Result<AppState, Box<dyn Error + Send + Sync>> {
        let config = AppConfig::new("127.0.0.1", 0, &self.base_url, BTreeMap::new());
        let agent_host = AgentHostClient::new(&self.base_url, Duration::from_secs(5))?;
        Ok(AppState::with_agent_host(config, agent_host))
    }

    fn recorded(&self) -> Result<Vec<RecordedRequest>, Box<dyn Error + Send + Sync>> {
        self.state.recorded()
    }

    fn clear_recorded(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.state.clear_recorded()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn format_base_url(address: SocketAddr) -> String {
    format!("http://{address}")
}

fn stub_router(state: StubState) -> Router {
    Router::new()
        .route(
            "/sessions",
            post(create_host_session).get(list_host_sessions),
        )
        .route("/sessions/{session_id}", get(get_host_session))
        .route(
            "/sessions/{session_id}/messages/stream",
            post(stream_host_message),
        )
        .route("/sessions/{session_id}/logs", get(host_logs))
        .route(
            "/sessions/{session_id}/container-logs",
            get(host_container_logs),
        )
        .route("/skills", post(create_host_skill).get(list_host_skills))
        .route("/skills/{skill_id}/versions", get(list_host_skill_versions))
        .route(
            "/skills/{skill_id}/versions/{version}/rollback",
            post(rollback_host_skill_version),
        )
        .route(
            "/skills/{skill_id}",
            get(get_host_skill)
                .put(update_host_skill)
                .delete(delete_host_skill),
        )
        .route("/gateways", post(create_host_gateway))
        .route("/gateways/{gateway_id}", delete(delete_host_gateway))
        .route("/gateways/{gateway_id}/logs", get(host_gateway_logs))
        .with_state(state)
}

async fn create_host_session(
    State(state): State<StubState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::POST, "/sessions", None, Some(body))?;
    Ok(Json(json!({
        "session_id": "host-session",
        "status": "running"
    })))
}

async fn list_host_sessions(
    State(state): State<StubState>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, "/sessions", query_string(query), None)?;
    Ok(Json(json!([
        {
            "session_id": "host-session",
            "status": "running",
            "stats": { "turns": 1 }
        },
        {
            "session_id": "orphan-host-session",
            "status": "idle"
        }
    ])))
}

async fn get_host_session(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, format!("/sessions/{session_id}"), None, None)?;
    Ok(Json(json!({
        "session_id": session_id,
        "status": "running"
    })))
}

async fn stream_host_message(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, StatusCode> {
    state.record(
        Method::POST,
        format!("/sessions/{session_id}/messages/stream"),
        None,
        Some(body),
    )?;
    let stream = concat!(
        "{\"type\":\"reasoning_delta\",\"content\":\"thinking\"}\n",
        "{\"type\":\"text_delta\",\"content\":\"Hello \"}\n",
        "{\"type\":\"tool_call\",\"tool\":\"shell\",\"input\":{\"cmd\":\"pwd\"}}\n",
        "{\"type\":\"tool_result\",\"tool\":\"shell\",\"output\":\"/repo\"}\n",
        "{\"type\":\"session/update\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"from stub\"}}}\n",
        "{\"type\":\"session/update\",\"update\":{\"sessionUpdate\":\"tool_call\",\"toolCallId\":\"call_1\",\"title\":\"Run tests\",\"kind\":\"command\",\"status\":\"pending\",\"rawInput\":{\"cmd\":\"pytest\"}}}\n",
        "{\"type\":\"session/update\",\"update\":{\"sessionUpdate\":\"tool_call_update\",\"toolCallId\":\"call_1\",\"status\":\"completed\",\"content\":[{\"type\":\"content\",\"content\":{\"type\":\"text\",\"text\":\"passed\"}}]}}\n",
    );
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from(stream),
    )
        .into_response())
}

async fn host_logs(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.record(
        Method::GET,
        format!("/sessions/{session_id}/logs"),
        None,
        None,
    )?;
    Ok(Json(json!({ "lines": ["kernel line 1", "kernel line 2"] })))
}

async fn host_container_logs(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    state.record(
        Method::GET,
        format!("/sessions/{session_id}/container-logs"),
        query_string(query),
        None,
    )?;
    Ok(Json(json!({ "lines": ["container line"] })))
}

async fn create_host_skill(
    State(state): State<StubState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::POST, "/skills", None, Some(body.clone()))?;
    Ok(Json(json!({
        "skill_id": body["skill_id"],
        "files": body["files"]
    })))
}

async fn list_host_skills(State(state): State<StubState>) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, "/skills", None, None)?;
    Ok(Json(json!([
        {
            "skill_id": "stub-skill",
            "files": { "SKILL.md": "content" }
        }
    ])))
}

async fn get_host_skill(State(state): State<StubState>, Path(skill_id): Path<String>) -> Response {
    if let Err(status) = state.record(Method::GET, format!("/skills/{skill_id}"), None, None) {
        return status.into_response();
    }
    if skill_id == "missing-skill" {
        return (StatusCode::NOT_FOUND, "skill not found").into_response();
    }
    Json(json!({
        "skill_id": skill_id,
        "files": { "SKILL.md": "content" }
    }))
    .into_response()
}

async fn update_host_skill(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if let Err(status) = state.record(
        Method::PUT,
        format!("/skills/{skill_id}"),
        None,
        Some(body.clone()),
    ) {
        return status.into_response();
    }
    if skill_id == "missing-skill" {
        return (StatusCode::NOT_FOUND, "skill not found").into_response();
    }
    Json(json!({
        "skill_id": skill_id,
        "files": body["files"]
    }))
    .into_response()
}

async fn list_host_skill_versions(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.record(
        Method::GET,
        format!("/skills/{skill_id}/versions"),
        None,
        None,
    )?;
    Ok(Json(json!([
        {
            "skill_id": skill_id,
            "version": 1,
            "created_at": "2026-01-01T00:00:00.000000Z",
            "files": { "SKILL.md": "# Version 1" }
        }
    ])))
}

async fn rollback_host_skill_version(
    State(state): State<StubState>,
    Path((skill_id, version)): Path<(String, u64)>,
) -> Result<Json<Value>, StatusCode> {
    state.record(
        Method::POST,
        format!("/skills/{skill_id}/versions/{version}/rollback"),
        None,
        None,
    )?;
    Ok(Json(json!({
        "skill_id": skill_id,
        "files": { "SKILL.md": "# Version 1" }
    })))
}

async fn delete_host_skill(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
) -> Response {
    if let Err(status) = state.record(Method::DELETE, format!("/skills/{skill_id}"), None, None) {
        return status.into_response();
    }
    if skill_id == "missing-skill" {
        return (StatusCode::NOT_FOUND, "skill not found").into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn create_host_gateway(State(state): State<StubState>, Json(body): Json<Value>) -> Response {
    if let Err(status) = state.record(Method::POST, "/gateways", None, Some(body.clone())) {
        return status.into_response();
    }
    if body["gateway_id"] == "failing-gateway" {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": "gateway failed to start" })),
        )
            .into_response();
    }
    Json(json!({
        "gateway_id": body["gateway_id"],
        "container_name": "gateway-container"
    }))
    .into_response()
}

async fn delete_host_gateway(
    State(state): State<StubState>,
    Path(gateway_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state.record(
        Method::DELETE,
        format!("/gateways/{gateway_id}"),
        None,
        None,
    )?;
    Ok(StatusCode::NO_CONTENT)
}

async fn host_gateway_logs(
    State(state): State<StubState>,
    Path(gateway_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.record(
        Method::GET,
        format!("/gateways/{gateway_id}/logs"),
        None,
        None,
    )?;
    Ok(Json(json!({ "lines": ["gateway line"] })))
}

fn query_string(query: BTreeMap<String, String>) -> Option<String> {
    if query.is_empty() {
        return None;
    }
    Some(
        query
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&"),
    )
}

async fn request_json(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    let (status, body) = request_raw(app, method, path, body).await?;
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    };
    Ok((status, value))
}

async fn request_raw(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<(StatusCode, Vec<u8>), Box<dyn Error + Send + Sync>> {
    let mut builder = Request::builder().method(method).uri(path);
    let request_body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body)?)
    } else {
        Body::empty()
    };
    let response = app.clone().oneshot(builder.body(request_body)?).await?;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await?.to_vec();
    Ok((status, body))
}

async fn post_json(
    app: &Router,
    path: &str,
    body: Value,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    request_json(app, Method::POST, path, Some(body)).await
}

async fn put_json(
    app: &Router,
    path: &str,
    body: Value,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    request_json(app, Method::PUT, path, Some(body)).await
}

async fn patch_json(
    app: &Router,
    path: &str,
    body: Value,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    request_json(app, Method::PATCH, path, Some(body)).await
}

async fn get_json(
    app: &Router,
    path: &str,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    request_json(app, Method::GET, path, None).await
}

async fn delete_json(
    app: &Router,
    path: &str,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    request_json(app, Method::DELETE, path, None).await
}

fn value_string<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a str, Box<dyn Error + Send + Sync>> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("expected string field {field:?} in {value}").into())
}

async fn create_basic_agent(app: &Router) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (status, _value) = post_json(
        app,
        "/agents",
        json!({
            "agent_id": "stub-agent",
            "name": "Stub Agent",
            "harness": "acp",
            "system_prompt": "Be helpful"
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    Ok(())
}

async fn create_basic_session(
    app: &Router,
) -> Result<(String, Value), Box<dyn Error + Send + Sync>> {
    let (status, value) = post_json(
        app,
        "/sessions",
        json!({
            "agent_id": "stub-agent",
            "channel_name": "cli",
            "client_type": "cli"
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    Ok((value_string(&value, "session_id")?.to_owned(), value))
}

#[tokio::test]
async fn create_session_merges_environment_for_agent_host()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    let (status, _config) = put_json(
        &app,
        "/kernel-configs/acp",
        json!({
            "env_vars": concat!(
                "SHARED=kernel\n",
                "KERNEL_ONLY=kernel\n",
                "CONNECTION_URL=http://kernel.example\n",
                "KERNEL_SYSTEM_PROMPT=kernel prompt\n"
            )
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _connection) = post_json(
        &app,
        "/connections",
        json!({
            "connection_id": "main-connection",
            "name": "Main Connection",
            "url": "http://connection.example",
            "provider_type": "azure",
            "api_flavor": "responses",
            "transport": "websockets",
            "azure_api_version": "2025-04-01-preview",
            "bearer_token": "connection-secret",
            "headers": {"x-provider-secret": "header-secret"}
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _workspace) = post_json(
        &app,
        "/workspaces",
        json!({ "workspace_id": "todo-list-code", "name": "TodoListCode" }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _agent) = post_json(
        &app,
        "/agents",
        json!({
            "agent_id": "stub-agent",
            "name": "Stub Agent",
            "harness": "acp",
            "system_prompt": "final system prompt",
            "skills": ["skill-a"],
            "workspace_mounts": [{ "workspace_id": "todo-list-code", "mode": "rw" }],
            "connection_id": "main-connection",
            "env_vars": concat!(
                "SHARED=agent\n",
                "AGENT_ONLY=agent\n",
                "CONNECTION_URL=http://agent-override.example\n",
                "CONNECTION_BEARER_TOKEN=agent-secret\n",
                "KERNEL_SYSTEM_PROMPT=agent env prompt\n"
            )
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _session) = post_json(
        &app,
        "/sessions",
        json!({
            "agent_id": "stub-agent",
            "channel_name": "web",
            "client_type": "webui"
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(
        server.recorded()?,
        vec![RecordedRequest {
            method: Method::POST,
            path: "/sessions".to_owned(),
            query: None,
            body: Some(json!({
                "harness": "acp",
                "skills": ["skill-a"],
                "env": {
                    "AGENTSPACE_AGENT_ID": "stub-agent",
                    "AGENTSPACE_CLIENT_SERVICE_URL": "http://client-service:8002",
                    "AGENT_ONLY": "agent",
                    "CONNECTION_AZURE_API_VERSION": "2025-04-01-preview",
                    "CONNECTION_API_FLAVOR": "responses",
                    "CONNECTION_BEARER_TOKEN": "agent-secret",
                    "CONNECTION_HEADERS": "{\"x-provider-secret\":\"header-secret\"}",
                    "CONNECTION_PROVIDER_TYPE": "azure",
                    "CONNECTION_TRANSPORT": "websockets",
                    "CONNECTION_URL": "http://agent-override.example",
                    "KERNEL_ONLY": "kernel",
                    "KERNEL_SYSTEM_PROMPT": "final system prompt",
                    "SHARED": "agent"
                },
                "workspace_mounts": [{ "workspace_id": "todo-list-code", "mode": "rw" }]
            })),
        }]
    );

    Ok(())
}

#[tokio::test]
async fn skill_routes_proxy_versions_and_auto_enable_creator()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    create_basic_agent(&app).await?;

    let (status, created) = post_json(
        &app,
        "/skills",
        json!({
            "skill_id": "agent-skill",
            "creator_agent_id": "stub-agent",
            "files": { "SKILL.md": "# Agent Skill" }
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["skill_id"], "agent-skill");

    let (status, agent) = get_json(&app, "/agents/stub-agent").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent["skills"], json!(["agent-skill"]));

    let (status, versions) = get_json(&app, "/skills/agent-skill/versions").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(versions[0]["version"], 1);

    let (status, rolled_back) =
        post_json(&app, "/skills/agent-skill/versions/1/rollback", json!({})).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rolled_back["files"]["SKILL.md"], "# Version 1");

    assert_eq!(
        server.recorded()?,
        vec![
            RecordedRequest {
                method: Method::POST,
                path: "/skills".to_owned(),
                query: None,
                body: Some(json!({
                    "skill_id": "agent-skill",
                    "files": { "SKILL.md": "# Agent Skill" }
                })),
            },
            RecordedRequest {
                method: Method::GET,
                path: "/skills/agent-skill/versions".to_owned(),
                query: None,
                body: None,
            },
            RecordedRequest {
                method: Method::POST,
                path: "/skills/agent-skill/versions/1/rollback".to_owned(),
                query: None,
                body: None,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn send_message_proxies_to_stream_and_persists_messages()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    create_basic_agent(&app).await?;
    let (session_id, _session) = create_basic_session(&app).await?;

    let (status, value) = post_json(
        &app,
        &format!("/sessions/{session_id}/messages"),
        json!({ "message": "Hello?" }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["assistant_message"]["content"], "Hello from stub");
    assert_eq!(value["assistant_message"]["reasoning"], "thinking");
    assert_eq!(
        value["assistant_message"]["tool_calls"],
        json!([
            {
                "tool": "shell",
                "input": "{\n  \"cmd\": \"pwd\"\n}",
                "output": "/repo",
                "content_offset": 5
            },
            {
                "tool": "Run tests",
                "tool_call_id": "call_1",
                "status": "completed",
                "kind": "command",
                "input": "{\n  \"cmd\": \"pytest\"\n}",
                "output": "passed",
                "content_offset": 15
            }
        ])
    );

    let (status, messages) = get_json(&app, &format!("/sessions/{session_id}/messages")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(messages["messages"][0]["role"], "user");
    assert_eq!(messages["messages"][0]["content"], "Hello?");
    assert_eq!(messages["messages"][1]["role"], "assistant");
    assert_eq!(messages["messages"][1]["content"], "Hello from stub");
    assert_eq!(
        messages["messages"][1]["tool_calls"],
        value["assistant_message"]["tool_calls"]
    );

    assert_eq!(
        server.recorded()?,
        vec![
            RecordedRequest {
                method: Method::POST,
                path: "/sessions".to_owned(),
                query: None,
                body: Some(json!({
                    "harness": "acp",
                    "env": {
                        "AGENTSPACE_AGENT_ID": "stub-agent",
                        "AGENTSPACE_CLIENT_SERVICE_URL": "http://client-service:8002",
                        "KERNEL_SYSTEM_PROMPT": "Be helpful"
                    },
                    "skills": []
                })),
            },
            RecordedRequest {
                method: Method::POST,
                path: "/sessions/host-session/messages/stream".to_owned(),
                query: None,
                body: Some(json!({ "message": "Hello?" })),
            },
            RecordedRequest {
                method: Method::GET,
                path: "/sessions/host-session".to_owned(),
                query: None,
                body: None,
            },
        ]
    );
    Ok(())
}

#[tokio::test]
async fn stream_message_returns_ndjson_events_and_final_payload()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    create_basic_agent(&app).await?;
    let (session_id, _session) = create_basic_session(&app).await?;

    let (status, body) = request_raw(
        &app,
        Method::POST,
        &format!("/sessions/{session_id}/messages/stream"),
        Some(json!({ "message": "Stream please" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let text = String::from_utf8(body)?;
    let lines = text
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(lines.len(), 8);
    assert_eq!(lines[0]["type"], "event");
    assert_eq!(lines[0]["event"]["type"], "reasoning_delta");
    assert_eq!(lines[1]["event"]["content"], "Hello ");
    assert_eq!(lines[2]["event"]["type"], "tool_call");
    assert_eq!(lines[5]["event"]["update"]["sessionUpdate"], "tool_call");
    assert_eq!(lines[7]["type"], "final");
    assert_eq!(lines[7]["completed"], true);
    assert_eq!(lines[7]["assistant_message"]["content"], "Hello from stub");
    assert_eq!(
        lines[7]["assistant_message"]["tool_calls"],
        json!([
            {
                "tool": "shell",
                "input": "{\n  \"cmd\": \"pwd\"\n}",
                "output": "/repo",
                "content_offset": 5
            },
            {
                "tool": "Run tests",
                "tool_call_id": "call_1",
                "status": "completed",
                "kind": "command",
                "input": "{\n  \"cmd\": \"pytest\"\n}",
                "output": "passed",
                "content_offset": 15
            }
        ])
    );

    Ok(())
}

#[tokio::test]
async fn kernels_include_client_session_metadata() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    create_basic_agent(&app).await?;
    let (session_id, _session) = create_basic_session(&app).await?;

    let (status, kernels) = get_json(&app, "/kernels").await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(kernels[0]["session_id"], "host-session");
    assert_eq!(kernels[0]["client_session_ids"], json!([session_id]));
    assert_eq!(kernels[0]["channel_names"], json!(["cli"]));
    assert_eq!(kernels[0]["agent_ids"], json!(["stub-agent"]));
    assert_eq!(kernels[1]["session_id"], "orphan-host-session");
    assert_eq!(kernels[1]["client_session_ids"], json!([]));
    assert_eq!(kernels[1]["channel_names"], json!([]));
    assert_eq!(kernels[1]["agent_ids"], json!([]));

    Ok(())
}

#[tokio::test]
async fn kernel_logs_and_container_logs_proxy_tail_and_all()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    let (status, kernel_logs) = get_json(&app, "/kernels/host-session/logs").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        kernel_logs,
        json!({ "lines": ["kernel line 1", "kernel line 2"] })
    );

    let (status, default_container_logs) =
        get_json(&app, "/kernels/host-session/container-logs").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        default_container_logs,
        json!({ "lines": ["container line"] })
    );

    let (status, tailed_container_logs) =
        get_json(&app, "/kernels/host-session/container-logs?tail=7").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        tailed_container_logs,
        json!({ "lines": ["container line"] })
    );

    let (status, all_container_logs) =
        get_json(&app, "/kernels/host-session/container-logs?all=true").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all_container_logs, json!({ "lines": ["container line"] }));

    assert_eq!(
        server.recorded()?,
        vec![
            RecordedRequest {
                method: Method::GET,
                path: "/sessions".to_owned(),
                query: None,
                body: None,
            },
            RecordedRequest {
                method: Method::GET,
                path: "/sessions/host-session/logs".to_owned(),
                query: None,
                body: None,
            },
            RecordedRequest {
                method: Method::GET,
                path: "/sessions".to_owned(),
                query: None,
                body: None,
            },
            RecordedRequest {
                method: Method::GET,
                path: "/sessions/host-session/container-logs".to_owned(),
                query: Some("tail=2000".to_owned()),
                body: None,
            },
            RecordedRequest {
                method: Method::GET,
                path: "/sessions".to_owned(),
                query: None,
                body: None,
            },
            RecordedRequest {
                method: Method::GET,
                path: "/sessions/host-session/container-logs".to_owned(),
                query: Some("tail=7".to_owned()),
                body: None,
            },
            RecordedRequest {
                method: Method::GET,
                path: "/sessions".to_owned(),
                query: None,
                body: None,
            },
            RecordedRequest {
                method: Method::GET,
                path: "/sessions/host-session/container-logs".to_owned(),
                query: None,
                body: None,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn skills_routes_proxy_crud_and_not_found_status() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let server = TestServer::start().await?;
    let app = server.app()?;

    let skill_body = json!({
        "skill_id": "stub-skill",
        "files": { "SKILL.md": "content" }
    });
    let (status, created) = post_json(&app, "/skills", skill_body).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(created["skill_id"], "stub-skill");

    let (status, skills) = get_json(&app, "/skills").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(skills[0]["skill_id"], "stub-skill");

    let (status, skill) = get_json(&app, "/skills/stub-skill").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(skill["files"]["SKILL.md"], "content");

    let (status, updated) = put_json(
        &app,
        "/skills/stub-skill",
        json!({ "files": { "SKILL.md": "updated" } }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["files"]["SKILL.md"], "updated");

    let (status, deleted) = delete_json(&app, "/skills/stub-skill").await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(deleted, Value::Null);

    let (status, missing_get) = get_json(&app, "/skills/missing-skill").await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        missing_get["detail"],
        "agent_host returned HTTP 404 Not Found"
    );

    let (status, missing_delete) = delete_json(&app, "/skills/missing-skill").await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        missing_delete["detail"],
        "agent_host returned HTTP 404 Not Found"
    );

    Ok(())
}

#[tokio::test]
async fn gateway_routes_proxy_and_update_persisted_status()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    create_basic_agent(&app).await?;

    let (status, gateway) = post_json(
        &app,
        "/gateways",
        json!({
            "gateway_id": "stub-gateway",
            "name": "Stub Gateway",
            "gateway_type": "echo",
            "agent_id": "stub-agent",
            "enabled": false,
            "env_vars": "VISIBLE=value",
            "secrets": { "SECRET": "secret-value" }
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gateway["status"], "stopped");
    assert_eq!(gateway["container_name"], Value::Null);

    let (status, started) = post_json(&app, "/gateways/stub-gateway/start", json!({})).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(started["status"], "running");
    assert_eq!(started["container_name"], "gateway-container");

    let (status, persisted_running) = get_json(&app, "/gateways/stub-gateway").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(persisted_running["status"], "running");
    assert_eq!(persisted_running["container_name"], "gateway-container");

    let (status, logs) = get_json(&app, "/gateways/stub-gateway/logs").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(logs, json!({ "lines": ["gateway line"] }));

    let (status, stopped) = post_json(&app, "/gateways/stub-gateway/stop", json!({})).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped["status"], "stopped");
    assert_eq!(stopped["container_name"], Value::Null);

    let (status, persisted_stopped) = get_json(&app, "/gateways/stub-gateway").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(persisted_stopped["status"], "stopped");
    assert_eq!(persisted_stopped["container_name"], Value::Null);

    assert_eq!(
        server.recorded()?,
        vec![
            RecordedRequest {
                method: Method::POST,
                path: "/gateways".to_owned(),
                query: None,
                body: Some(json!({
                    "gateway_id": "stub-gateway",
                    "gateway_type": "echo",
                    "agent_id": "stub-agent",
                    "env": {
                        "SECRET": "secret-value",
                        "VISIBLE": "value"
                    }
                })),
            },
            RecordedRequest {
                method: Method::GET,
                path: "/gateways/stub-gateway/logs".to_owned(),
                query: None,
                body: None,
            },
            RecordedRequest {
                method: Method::DELETE,
                path: "/gateways/stub-gateway".to_owned(),
                query: None,
                body: None,
            },
        ]
    );

    Ok(())
}

#[tokio::test]
async fn gateway_enable_persists_when_runtime_start_fails()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    create_basic_agent(&app).await?;

    let (status, _gateway) = post_json(
        &app,
        "/gateways",
        json!({
            "gateway_id": "failing-gateway",
            "name": "Failing Gateway",
            "gateway_type": "echo",
            "agent_id": "stub-agent",
            "enabled": false
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, gateway) = patch_json(
        &app,
        "/gateways/failing-gateway",
        json!({ "enabled": true }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gateway["enabled"], true);
    assert_eq!(gateway["status"], "error");
    assert_eq!(
        gateway["last_error"],
        "agent_host returned HTTP 500 Internal Server Error"
    );

    let (status, persisted) = get_json(&app, "/gateways/failing-gateway").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(persisted["enabled"], true);
    assert_eq!(persisted["status"], "error");

    Ok(())
}

#[tokio::test]
async fn enabled_stopped_gateways_autostart() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let state = server.app_state()?;
    let app = build_router(state.clone());
    create_basic_agent(&app).await?;

    let (status, _gateway) = post_json(
        &app,
        "/gateways",
        json!({
            "gateway_id": "autostart-gateway",
            "name": "Autostart Gateway",
            "gateway_type": "echo",
            "agent_id": "stub-agent",
            "enabled": false,
            "env_vars": "VISIBLE=value",
            "secrets": { "SECRET": "secret-value" }
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, enabled) = patch_json(
        &app,
        "/gateways/autostart-gateway",
        json!({ "enabled": true }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(enabled["enabled"], true);
    assert_eq!(enabled["status"], "running");

    let (status, stopped) = post_json(&app, "/gateways/autostart-gateway/stop", json!({})).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped["enabled"], true);
    assert_eq!(stopped["status"], "stopped");

    server.clear_recorded()?;
    start_enabled_gateways(state).await;

    let (status, gateway) = get_json(&app, "/gateways/autostart-gateway").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gateway["enabled"], true);
    assert_eq!(gateway["status"], "running");
    assert_eq!(gateway["container_name"], "gateway-container");
    assert_eq!(
        server.recorded()?,
        vec![RecordedRequest {
            method: Method::POST,
            path: "/gateways".to_owned(),
            query: None,
            body: Some(json!({
                "gateway_id": "autostart-gateway",
                "gateway_type": "echo",
                "agent_id": "stub-agent",
                "env": {
                    "SECRET": "secret-value",
                    "VISIBLE": "value"
                }
            })),
        }]
    );

    Ok(())
}
