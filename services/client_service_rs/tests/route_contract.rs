#![allow(clippy::too_many_lines)]

use std::{collections::BTreeMap, error::Error, io, time::Duration};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::Path,
    http::{Method, Request, StatusCode},
    routing::{get, post},
};
use client_service_rs::{
    AppConfig, AppState, agent_host::AgentHostClient, build_router,
    models::DEFAULT_AGENT_SYSTEM_PROMPT,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;

struct StubAgentHost {
    base_url: String,
    handle: JoinHandle<()>,
}

impl Drop for StubAgentHost {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn test_router(agent_host_base_url: &str) -> Result<Router, Box<dyn Error + Send + Sync>> {
    let config = AppConfig::new("127.0.0.1", 0, agent_host_base_url, BTreeMap::new());
    let agent_host = AgentHostClient::new(agent_host_base_url, Duration::from_millis(100))?;
    Ok(build_router(AppState::with_agent_host(config, agent_host)?))
}

async fn spawn_stub_agent_host() -> Result<StubAgentHost, Box<dyn Error + Send + Sync>> {
    let app = Router::new()
        .route("/sessions", post(stub_create_session))
        .route(
            "/sessions/{session_id}",
            get(stub_get_session).delete(stub_delete_session),
        )
        .route(
            "/sessions/{session_id}/workspace/snapshot",
            post(stub_snapshot_workspace),
        )
        .route("/workspaces/clone", post(stub_clone_workspace))
        .route("/workspaces/vscode", post(stub_workspace_vscode));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("stub agent_host failed: {error}");
        }
    });

    Ok(StubAgentHost {
        base_url: format!("http://{address}"),
        handle,
    })
}

async fn stub_create_session(Json(payload): Json<Value>) -> Json<Value> {
    Json(json!({ "session_id": payload["session_id"], "status": "idle" }))
}

async fn stub_get_session(Path(session_id): Path<String>) -> Json<Value> {
    Json(json!({ "session_id": session_id, "status": "idle" }))
}

async fn stub_delete_session(Path(_session_id): Path<String>) -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn stub_snapshot_workspace(
    Path(session_id): Path<String>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    Json(json!({
        "session_id": session_id,
        "workspace_id": payload["workspace_id"],
        "volume_name": payload["volume_name"],
        "exclude_names": payload["exclude_names"],
    }))
}

async fn stub_clone_workspace(Json(payload): Json<Value>) -> Json<Value> {
    Json(json!({
        "workspace_id": payload["target_workspace_id"],
        "volume_name": payload["target_volume_name"],
    }))
}

async fn stub_workspace_vscode(Json(payload): Json<Value>) -> Json<Value> {
    Json(json!({
        "workspace_id": payload["workspace_id"],
        "volume_name": payload["volume_name"],
        "container_name": format!("editor-{}", payload["workspace_id"].as_str().unwrap_or_default()),
        "vscode_url": "http://127.0.0.1:45678",
    }))
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
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()))
    };
    Ok((status, value))
}

async fn get_json(
    app: Router,
    path: &str,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    request_json(app, Method::GET, path, None).await
}

fn assert_error_detail(value: &Value) {
    assert!(value.get("detail").is_some_and(Value::is_string));
}

fn string_field(value: &Value, field: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| io::Error::other(format!("missing string field {field}")).into())
}

#[tokio::test]
async fn basic_routes_and_kernel_configs_match_contract() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let app = test_router("http://127.0.0.1:9")?;

    let (status, value) = get_json(app.clone(), "/healthz").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({ "status": "ok" }));

    let (status, value) = get_json(app.clone(), "/harnesses").await?;
    assert_eq!(status, StatusCode::OK);
    let harnesses = value
        .as_array()
        .ok_or_else(|| io::Error::other("harness list was not an array"))?;
    assert!(harnesses.iter().all(Value::is_string));
    assert!(harnesses.contains(&json!("acp")));

    let (status, value) = get_json(app.clone(), "/gateway-types").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!(["echo", "discord"]));

    let (status, value) = get_json(app.clone(), "/kernel-configs").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!([]));

    let (status, value) = get_json(app.clone(), "/kernel-configs/acp").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value,
        json!({ "harness": "acp", "env_vars": "", "updated_at": null })
    );

    let (status, updated) = request_json(
        app.clone(),
        Method::PUT,
        "/kernel-configs/acp",
        Some(json!({ "env_vars": "A=B\nC=D" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["harness"], "acp");
    assert_eq!(updated["env_vars"], "A=B\nC=D");
    assert!(updated["updated_at"].is_string());

    let (status, value) = get_json(app.clone(), "/kernel-configs").await?;
    assert_eq!(status, StatusCode::OK);
    let configs = value
        .as_array()
        .ok_or_else(|| io::Error::other("kernel config list was not an array"))?;
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0], updated);

    let (status, value) = get_json(app, "/kernel-configs/missing").await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_detail(&value);

    Ok(())
}

#[tokio::test]
async fn save_session_workspace_marks_workspace_ready() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let stub = spawn_stub_agent_host().await?;
    let app = test_router(&stub.base_url)?;

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
        Some(json!({
            "agent_id": "agent-one",
            "channel_name": "webui",
            "client_type": "webui",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let session_id = string_field(&session, "session_id")?;

    let (status, workspace) = request_json(
        app.clone(),
        Method::POST,
        &format!("/sessions/{session_id}/workspace/save"),
        Some(json!({ "workspace_id": "saved-workspace", "name": "Saved Workspace" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workspace["workspace_id"], "saved-workspace");
    assert_eq!(workspace["status"], "ready");
    assert_eq!(workspace["mount_path"], "/workspace/saved-workspace");
    assert_eq!(
        workspace["volume_name"],
        "agentspace-workspace-saved-workspace"
    );

    let (status, workspaces) = get_json(app, "/workspaces").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workspaces[0]["status"], "ready");

    Ok(())
}

#[tokio::test]
async fn workspace_clone_and_vscode_routes_match_contract()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let stub = spawn_stub_agent_host().await?;
    let app = test_router(&stub.base_url)?;

    let (status, source) = request_json(
        app.clone(),
        Method::POST,
        "/workspaces",
        Some(json!({ "workspace_id": "source-workspace", "name": "Source Workspace" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(source["status"], "ready");

    let (status, cloned) = request_json(
        app.clone(),
        Method::POST,
        "/workspaces/source-workspace/clone",
        Some(json!({ "workspace_id": "cloned-workspace", "name": "Cloned Workspace" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cloned["workspace_id"], "cloned-workspace");
    assert_eq!(cloned["name"], "Cloned Workspace");
    assert_eq!(cloned["status"], "ready");
    assert_eq!(
        cloned["volume_name"],
        "agentspace-workspace-cloned-workspace"
    );
    assert_eq!(cloned["mount_path"], "/workspace/cloned-workspace");

    let (status, vscode) = request_json(
        app,
        Method::POST,
        "/workspaces/cloned-workspace/vscode",
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(vscode["workspace_id"], "cloned-workspace");
    assert_eq!(
        vscode["volume_name"],
        "agentspace-workspace-cloned-workspace"
    );
    assert_eq!(vscode["container_name"], "editor-cloned-workspace");
    assert_eq!(vscode["vscode_url"], "http://127.0.0.1:45678");

    Ok(())
}

#[tokio::test]
async fn connection_routes_match_contract() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = test_router("http://127.0.0.1:9")?;
    let connection = json!({
        "connection_id": "main",
        "name": "Main",
        "url": "http://models.example.test",
        "api_flavor": "responses",
        "api_key": "secret",
    });

    let (status, value) = get_json(app.clone(), "/connections").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!([]));

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/connections",
        Some(connection.clone()),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["connection_id"], "main");
    assert_eq!(value["api_key"], "secret");
    assert_eq!(value["has_api_key"], json!(true));

    let (status, value) =
        request_json(app.clone(), Method::POST, "/connections", Some(connection)).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_error_detail(&value);

    let (status, value) = get_json(app.clone(), "/connections/main").await?;
    assert_eq!(status, StatusCode::OK);
    assert!(value.get("api_key").is_none());
    assert_eq!(value["api_flavor"], "responses");
    assert_eq!(value["has_api_key"], json!(true));

    let (status, value) = get_json(app.clone(), "/connections").await?;
    assert_eq!(status, StatusCode::OK);
    let connections = value
        .as_array()
        .ok_or_else(|| io::Error::other("connection list was not an array"))?;
    assert_eq!(connections.len(), 1);
    assert!(connections[0].get("api_key").is_none());
    assert_eq!(connections[0]["has_api_key"], json!(true));

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/connections/main",
        Some(json!({
            "name": "Renamed",
            "url": "http://renamed.example.test",
            "api_flavor": "chat_completions",
            "api_key": "",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["name"], "Renamed");
    assert_eq!(value["api_key"], "");
    assert_eq!(value["has_api_key"], json!(false));

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/connections",
        Some(json!({ "connection_id": "Bad", "name": "Bad", "url": "x" })),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/connections/missing",
        Some(json!({ "name": "Nope" })),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    let (status, value) =
        request_json(app.clone(), Method::DELETE, "/connections/main", None).await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(value, Value::Null);

    let (status, value) = request_json(app, Method::DELETE, "/connections/main", None).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    Ok(())
}

#[tokio::test]
async fn agent_routes_match_contract() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = test_router("http://127.0.0.1:9")?;
    let (status, _value) = request_json(
        app.clone(),
        Method::POST,
        "/connections",
        Some(json!({
            "connection_id": "main",
            "name": "Main",
            "url": "http://models.example.test",
            "api_key": "secret",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

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
            "connection_id": "main",
            "cli": {
                "harness": "copilot-cli",
                "connection_id": "main",
            },
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["harness"], "acp");
    assert_eq!(value["connection_id"], "main");
    assert_eq!(value["cli"]["harness"], "copilot-cli");
    assert_eq!(value["cli"]["connection_id"], "main");

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/agents",
        Some(json!({ "agent_id": "agent-two", "name": "Agent Two", "harness": "echo" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["harness"], "echo");
    assert_eq!(value["system_prompt"], DEFAULT_AGENT_SYSTEM_PROMPT);

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/agents",
        Some(json!({ "agent_id": "agent-one", "name": "Duplicate" })),
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/agents",
        Some(json!({ "agent_id": "Bad", "name": "Bad" })),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/agents",
        Some(json!({ "agent_id": "agent-three", "name": "Bad", "connection_id": "missing" })),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/agents/agent-one",
        Some(json!({ "name": "Renamed" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["name"], "Renamed");
    assert_eq!(value["system_prompt"], "help");
    assert_eq!(value["skills"], json!(["skill-a"]));
    assert_eq!(value["env_vars"], "A=B");
    assert_eq!(value["connection_id"], "main");
    assert_eq!(value["cli"]["connection_id"], "main");

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/agents/agent-one",
        Some(json!({ "connection_id": null })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert!(value["connection_id"].is_null());
    assert_eq!(value["cli"]["connection_id"], "main");

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/agents/agent-one",
        Some(json!({ "cli": null })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert!(value["cli"].is_null());

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/agents/agent-one",
        Some(json!({ "connection_id": "missing" })),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/agents/agent-one",
        Some(json!({
            "cli": {
                "harness": "copilot-cli",
                "connection_id": "missing",
            },
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/agents/agent-one",
        Some(json!({ "cli": { "harness": "acp" } })),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(value.is_string() || value.get("detail").is_some());

    let (status, value) = get_json(app.clone(), "/agents/missing").await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    let (status, value) = get_json(app.clone(), "/agents").await?;
    assert_eq!(status, StatusCode::OK);
    let agents = value
        .as_array()
        .ok_or_else(|| io::Error::other("agent list was not an array"))?;
    assert_eq!(agents.len(), 2);

    let (status, value) = request_json(app, Method::DELETE, "/agents/missing", None).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    Ok(())
}

#[tokio::test]
async fn workspace_routes_and_agent_mounts_match_contract()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = test_router("http://127.0.0.1:9")?;

    let (status, value) = get_json(app.clone(), "/workspaces").await?;
    assert_eq!(status, StatusCode::OK);
    assert!(value.as_array().is_some_and(Vec::is_empty));

    let (status, workspace) = request_json(
        app.clone(),
        Method::POST,
        "/workspaces",
        Some(json!({ "workspace_id": "todo-list-code", "name": "TodoListCode" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workspace["workspace_id"], "todo-list-code");
    assert_eq!(workspace["mount_path"], "/workspace/todo-list-code");
    assert_eq!(
        workspace["volume_name"],
        "agentspace-workspace-todo-list-code"
    );

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/workspaces",
        Some(json!({ "workspace_id": "Bad", "name": "Bad" })),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_detail(&value);

    let (status, renamed) = request_json(
        app.clone(),
        Method::PATCH,
        "/workspaces/todo-list-code",
        Some(json!({ "name": "RenamedCode" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["name"], "RenamedCode");

    let (status, agent) = request_json(
        app.clone(),
        Method::POST,
        "/agents",
        Some(json!({
            "agent_id": "agent-one",
            "name": "Agent One",
            "workspace_mounts": [{ "workspace_id": "todo-list-code", "mode": "ro" }],
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        agent["workspace_mounts"],
        json!([{
            "workspace_id": "todo-list-code",
            "mode": "ro",
            "mount_path": "/workspace/todo-list-code",
            "volume_name": null
        }])
    );

    let (status, value) = request_json(
        app.clone(),
        Method::DELETE,
        "/workspaces/todo-list-code",
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_error_detail(&value);

    let (status, agent) = request_json(
        app.clone(),
        Method::PATCH,
        "/agents/agent-one",
        Some(json!({ "workspace_mounts": [] })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent["workspace_mounts"], json!([]));

    let (status, value) = request_json(
        app.clone(),
        Method::DELETE,
        "/workspaces/todo-list-code",
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(value.is_null());

    Ok(())
}

#[tokio::test]
async fn stopped_gateway_routes_match_contract() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = test_router("http://127.0.0.1:9")?;
    let (status, _value) = request_json(
        app.clone(),
        Method::POST,
        "/agents",
        Some(json!({ "agent_id": "agent-one", "name": "Agent One" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let gateway = json!({
        "gateway_id": "gateway-one",
        "name": "Gateway One",
        "gateway_type": "discord",
        "agent_id": "agent-one",
        "enabled": false,
        "env_vars": "A=B",
        "secrets": { "Z_TOKEN": "last", "A_TOKEN": "first" },
    });
    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/gateways",
        Some(gateway.clone()),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["gateway_id"], "gateway-one");
    assert_eq!(value["status"], "stopped");
    assert_eq!(value["secret_keys"], json!(["A_TOKEN", "Z_TOKEN"]));
    assert!(value.get("secrets").is_none());

    let (status, value) =
        request_json(app.clone(), Method::POST, "/gateways", Some(gateway)).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_error_detail(&value);

    let (status, value) = get_json(app.clone(), "/gateways/gateway-one").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["enabled"], json!(false));
    assert!(value.get("secrets").is_none());

    let (status, value) = get_json(app.clone(), "/gateways").await?;
    assert_eq!(status, StatusCode::OK);
    let gateways = value
        .as_array()
        .ok_or_else(|| io::Error::other("gateway list was not an array"))?;
    assert_eq!(gateways.len(), 1);
    assert_eq!(gateways[0]["secret_keys"], json!(["A_TOKEN", "Z_TOKEN"]));
    assert!(gateways[0].get("secrets").is_none());

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/gateways/gateway-one",
        Some(json!({ "name": "Renamed", "secrets": { "M_TOKEN": "middle" } })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["name"], "Renamed");
    assert_eq!(
        value["secret_keys"],
        json!(["A_TOKEN", "M_TOKEN", "Z_TOKEN"])
    );
    assert!(value.get("secrets").is_none());

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/gateways",
        Some(json!({
            "gateway_id": "Bad",
            "name": "Bad",
            "gateway_type": "echo",
            "agent_id": "agent-one",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/gateways",
        Some(json!({
            "gateway_id": "gateway-two",
            "name": "Gateway Two",
            "gateway_type": "echo",
            "agent_id": "missing",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app.clone(),
        Method::PATCH,
        "/gateways/gateway-one",
        Some(json!({ "agent_id": "missing" })),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    let (status, value) =
        request_json(app.clone(), Method::DELETE, "/gateways/gateway-one", None).await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(value, Value::Null);

    let (status, value) = get_json(app, "/gateways/gateway-one").await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    Ok(())
}

#[tokio::test]
async fn missing_session_routes_return_fastapi_style_errors_without_upstream()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = test_router("http://127.0.0.1:9")?;

    for (method, path) in [
        (Method::GET, "/sessions/missing"),
        (Method::GET, "/sessions/missing/messages"),
        (Method::POST, "/sessions/missing/messages"),
        (Method::POST, "/sessions/missing/messages/stream"),
        (Method::GET, "/sessions/missing/turns/turn-one/stream"),
        (Method::POST, "/sessions/missing/reset"),
        (Method::DELETE, "/sessions/missing"),
    ] {
        let body = if method == Method::POST && path.contains("messages") {
            Some(json!({ "message": "hello" }))
        } else {
            None
        };
        let (status, value) = request_json(app.clone(), method, path, body).await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_error_detail(&value);
    }

    Ok(())
}

#[tokio::test]
async fn created_session_message_listing_shape_matches_contract()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let stub = spawn_stub_agent_host().await?;
    let app = test_router(&stub.base_url)?;

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
        Some(json!({
            "agent_id": "agent-one",
            "channel_name": "webui",
            "client_type": "webui",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let session_id = string_field(&session, "session_id")?;
    assert_eq!(session["agent_id"], "agent-one");
    assert_eq!(session["agent_host_session_id"], session_id);
    assert_eq!(session["status"], "idle");
    assert_eq!(session["interaction_mode"], "chat");
    assert_eq!(session["recovery_state"], "recoverable");
    assert_eq!(session["workspace_volume_identity"], session_id);
    assert!(session["cli_harness"].is_null());
    assert_eq!(session["message_count"], json!(0));

    let (status, value) =
        get_json(app.clone(), &format!("/sessions/{session_id}/messages")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value, json!({ "messages": [] }));

    let (status, value) = get_json(app.clone(), "/sessions").await?;
    assert_eq!(status, StatusCode::OK);
    let sessions = value
        .as_array()
        .ok_or_else(|| io::Error::other("session list was not an array"))?;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["message_count"], json!(0));

    let (status, value) = get_json(app.clone(), &format!("/sessions/{session_id}")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["messages"], json!([]));

    let (status, value) = request_json(
        app.clone(),
        Method::DELETE,
        &format!("/sessions/{session_id}"),
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(value, Value::Null);

    let (status, value) = get_json(app, &format!("/sessions/{session_id}")).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_error_detail(&value);

    Ok(())
}

#[tokio::test]
async fn cli_sessions_are_durable_starting_records_without_upstream_runtime()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = test_router("http://127.0.0.1:9")?;
    let (status, _connection) = request_json(
        app.clone(),
        Method::POST,
        "/connections",
        Some(json!({
            "connection_id": "openrouter",
            "name": "OpenRouter",
            "url": "https://openrouter.ai/api/v1",
            "api_flavor": "responses",
            "api_key": "must-not-be-snapshotted",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _agent) = request_json(
        app.clone(),
        Method::POST,
        "/agents",
        Some(json!({
            "agent_id": "cli-agent",
            "name": "CLI Agent",
            "system_prompt": "Review this workspace.",
            "env_vars": "COPILOT_MODEL=gpt-5.4\nCOPILOT_REASONING_EFFORT=high",
            "cli": {
                "harness": "copilot-cli",
                "connection_id": "openrouter",
            },
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let (status, session) = request_json(
        app.clone(),
        Method::POST,
        "/sessions",
        Some(json!({
            "agent_id": "cli-agent",
            "channel_name": "webui",
            "client_type": "webui",
            "interaction_mode": "cli",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session["interaction_mode"], "cli");
    assert_eq!(session["status"], "starting");
    assert_eq!(session["runtime_status"], "starting");
    assert_eq!(session["runtime_generation"], 0);
    assert_eq!(session["cli_harness"], "copilot-cli");
    assert_eq!(session["cli_connection_id"], "openrouter");
    assert!(session["agent_host_session_id"].is_null());
    assert_eq!(session["recovery_state"], "recoverable");
    assert_eq!(
        session["launch_snapshot"]["provider"]["provider_type"],
        "openai"
    );
    assert_eq!(
        session["launch_snapshot"]["provider"]["wire_api"],
        "responses"
    );
    assert_eq!(
        session["launch_snapshot"]["provider"]["api_key"]["kind"],
        "config_reference"
    );
    assert_eq!(session["launch_snapshot"]["model"]["value"], "gpt-5.4");
    assert_eq!(
        session["launch_snapshot"]["reasoning_effort"]["value"],
        "high"
    );
    let harness_session_id = string_field(&session, "harness_session_id")?;
    uuid::Uuid::parse_str(&harness_session_id)?;
    assert!(
        !serde_json::to_string(&session)?.contains("must-not-be-snapshotted"),
        "CLI launch snapshot persisted a credential"
    );

    let session_id = string_field(&session, "session_id")?;
    let (status, detail) = get_json(app.clone(), &format!("/sessions/{session_id}")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["interaction_mode"], "cli");
    assert_eq!(detail["messages"], json!([]));

    for (method, path, body) in [
        (
            Method::GET,
            format!("/sessions/{session_id}/messages"),
            None,
        ),
        (
            Method::POST,
            format!("/sessions/{session_id}/messages"),
            Some(json!({ "message": "hello" })),
        ),
        (Method::POST, format!("/sessions/{session_id}/reset"), None),
    ] {
        let (status, value) = request_json(app.clone(), method, &path, body).await?;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_error_detail(&value);
    }

    let (status, _plain_agent) = request_json(
        app.clone(),
        Method::POST,
        "/agents",
        Some(json!({ "agent_id": "chat-only", "name": "Chat Only" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, value) = request_json(
        app.clone(),
        Method::POST,
        "/sessions",
        Some(json!({
            "agent_id": "chat-only",
            "interaction_mode": "cli",
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_error_detail(&value);

    let (status, value) = request_json(
        app,
        Method::DELETE,
        &format!("/sessions/{session_id}"),
        None,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(value, Value::Null);
    Ok(())
}
