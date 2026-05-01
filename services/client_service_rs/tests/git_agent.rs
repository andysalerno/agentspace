use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{Method, Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use client_service_rs::{
    AppConfig, AppState, agent_host::AgentHostClient, build_router, git_agent::GitAgentClient,
    store::StoreSet,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;

struct StubGitAgent {
    base_url: String,
    handle: JoinHandle<Result<(), std::io::Error>>,
}

struct StubAgentHost {
    base_url: String,
    handle: JoinHandle<Result<(), std::io::Error>>,
    recorded: Arc<Mutex<Vec<Value>>>,
}

impl StubAgentHost {
    async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/sessions", post(stub_create_session))
            .route("/workspaces/vscode", post(stub_open_workspace_vscode))
            .with_state(Arc::clone(&recorded));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let handle = tokio::spawn(axum::serve(listener, app).into_future());
        Ok(Self {
            base_url: format!("http://{address}"),
            handle,
            recorded,
        })
    }

    fn recorded(&self) -> Result<Vec<Value>, Box<dyn Error + Send + Sync>> {
        self.recorded
            .lock()
            .map(|guard| guard.clone())
            .map_err(|_error| "recorded request lock poisoned".into())
    }
}

impl Drop for StubAgentHost {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl StubGitAgent {
    async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let app = Router::new()
            .route("/status", get(stub_status))
            .route("/patch-requests", get(stub_list_requests))
            .route("/patch-requests/{request_id}", get(stub_get_request))
            .route(
                "/patch-requests/{request_id}/rerun-review",
                post(stub_rerun_review),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let handle = tokio::spawn(axum::serve(listener, app).into_future());
        Ok(Self {
            base_url: format!("http://{address}"),
            handle,
        })
    }
}

impl Drop for StubGitAgent {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn test_router(git_agent_base_url: &str) -> Result<Router, Box<dyn Error + Send + Sync>> {
    test_router_with_stores(git_agent_base_url, StoreSet::in_memory())
}

fn test_router_with_stores(
    git_agent_base_url: &str,
    stores: StoreSet,
) -> Result<Router, Box<dyn Error + Send + Sync>> {
    test_router_with_agent_host_and_stores(git_agent_base_url, "http://127.0.0.1:9", stores)
}

fn test_router_with_agent_host_and_stores(
    git_agent_base_url: &str,
    agent_host_base_url: &str,
    stores: StoreSet,
) -> Result<Router, Box<dyn Error + Send + Sync>> {
    let config = AppConfig::new("127.0.0.1", 0, agent_host_base_url, BTreeMap::new())
        .with_git_agent_base_url(git_agent_base_url)
        .with_git_agent_data_volume_name("custom-git-agent-data");
    let agent_host = AgentHostClient::new(agent_host_base_url, Duration::from_millis(100))?;
    let git_agent = GitAgentClient::new(git_agent_base_url, Duration::from_secs(5));
    Ok(build_router(AppState::with_clients_and_stores(
        config, agent_host, git_agent, stores,
    )))
}

async fn stub_status() -> Json<Value> {
    Json(json!({
        "status": "ready",
        "default_branch": "main",
        "request_count": 1,
    }))
}

async fn stub_list_requests() -> Json<Value> {
    Json(json!([
        {
            "request_id": "request-one",
            "status": "accepted",
            "target_ref": "refs/heads/main",
        }
    ]))
}

async fn stub_get_request(Path(request_id): Path<String>) -> impl IntoResponse {
    if request_id == "missing" {
        return (StatusCode::NOT_FOUND, "missing request").into_response();
    }
    Json(json!({
        "request_id": request_id,
        "status": "accepted",
        "raw_patch": "diff --git a/README.md b/README.md",
        "patch": {
            "files": [
                { "path": "README.md", "additions": 1, "deletions": 0 }
            ]
        }
    }))
    .into_response()
}

async fn stub_rerun_review(Path(request_id): Path<String>) -> Json<Value> {
    Json(json!({
        "request_id": request_id,
        "status": "reviewing",
        "rerun": true,
    }))
}

async fn stub_create_session(
    State(recorded): State<Arc<Mutex<Vec<Value>>>>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    recorded
        .lock()
        .map_err(|_error| StatusCode::INTERNAL_SERVER_ERROR)?
        .push(payload);
    Ok(Json(json!({
        "session_id": "agent-host-session",
        "status": "idle",
    })))
}

async fn stub_open_workspace_vscode(
    State(recorded): State<Arc<Mutex<Vec<Value>>>>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    recorded
        .lock()
        .map_err(|_error| StatusCode::INTERNAL_SERVER_ERROR)?
        .push(payload.clone());
    Ok(Json(json!({
        "workspace_id": payload["workspace_id"],
        "volume_name": payload["volume_name"],
        "container_name": "workspace-editor",
        "vscode_url": "http://127.0.0.1:49152",
    })))
}

async fn request_json(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    let mut builder = Request::builder().method(method).uri(path);
    let request_body = if let Some(body) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(serde_json::to_vec(&body)?)
    } else {
        Body::empty()
    };
    let response = app.clone().oneshot(builder.body(request_body)?).await?;
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
    app: &Router,
    path: &str,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    request_json(app, Method::GET, path, None).await
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

async fn post_json(
    app: &Router,
    path: &str,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    request_json(app, Method::POST, path, Some(json!({}))).await
}

fn sqlite_test_path() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("sqlite-tests");
    fs::create_dir_all(&directory)?;
    Ok(directory.join(format!("{}.db", uuid::Uuid::now_v7().simple())))
}

fn cleanup_sqlite_path(path: &FsPath) {
    let raw = path.to_string_lossy();
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{raw}-wal")),
        PathBuf::from(format!("{raw}-shm")),
    ] {
        let _ignored = fs::remove_file(candidate);
    }
}

#[tokio::test]
async fn git_agent_config_get_update_and_reserved_agent_work()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = test_router("http://127.0.0.1:9")?;

    let (status, config) = get_json(&app, "/git-agent/config").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(config["enabled"], true);
    assert_eq!(config["default_branch"], "main");
    assert_eq!(config["allowed_refs"], json!(["refs/heads/main"]));
    assert_eq!(config["allowed_ref_prefixes"], json!(["refs/heads/wip/"]));
    assert_eq!(config["review_agent_id"], "git-agent");
    assert_eq!(config["validation_command"], "just validate");

    let (status, agent) = get_json(&app, "/agents/git-agent").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent["agent_id"], "git-agent");
    assert_eq!(agent["harness"], "acp");

    let (status, updated_agent) = patch_json(
        &app,
        "/agents/git-agent",
        json!({ "name": "Custom Git Reviewer" }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated_agent["name"], "Custom Git Reviewer");

    let (status, updated) = put_json(
        &app,
        "/git-agent/config",
        json!({
            "enabled": false,
            "default_branch": "trunk",
            "allowed_ref_prefixes": ["refs/heads/wip/", "refs/heads/dev/"],
            "allowed_refs": ["refs/heads/trunk"],
            "remote_url": "http://gitagent.example/repo.git",
            "patch_url": "http://gitagent.example/PatchRequest",
            "validation_command": "just validate-all"
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["enabled"], false);
    assert_eq!(updated["default_branch"], "trunk");
    assert_eq!(updated["allowed_refs"], json!(["refs/heads/trunk"]));
    assert_eq!(updated["validation_command"], "just validate-all");

    let (status, persisted) = get_json(&app, "/git-agent/config").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(persisted, updated);

    let (status, preserved_agent) = get_json(&app, "/agents/git-agent").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preserved_agent["name"], "Custom Git Reviewer");

    let (status, error) = put_json(
        &app,
        "/git-agent/config",
        json!({ "allowed_refs": ["main"] }),
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(error["detail"].is_string());

    Ok(())
}

#[tokio::test]
async fn git_agent_config_persists_in_sqlite() -> Result<(), Box<dyn Error + Send + Sync>> {
    let path = sqlite_test_path()?;
    {
        let app = test_router_with_stores("http://127.0.0.1:9", StoreSet::sqlite(&path)?)?;
        let (status, _config) = put_json(
            &app,
            "/git-agent/config",
            json!({
                "enabled": false,
                "default_branch": "trunk",
                "allowed_ref_prefixes": ["refs/heads/dev/"],
                "allowed_refs": ["refs/heads/trunk"],
                "remote_url": "http://gitagent.example/repo.git",
                "patch_url": "http://gitagent.example/PatchRequest",
                "validation_command": "just verify"
            }),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
    }

    {
        let app = test_router_with_stores("http://127.0.0.1:9", StoreSet::sqlite(&path)?)?;
        let (status, config) = get_json(&app, "/git-agent/config").await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(config["enabled"], false);
        assert_eq!(config["default_branch"], "trunk");
        assert_eq!(config["allowed_ref_prefixes"], json!(["refs/heads/dev/"]));
        assert_eq!(config["allowed_refs"], json!(["refs/heads/trunk"]));
        assert_eq!(config["validation_command"], "just verify");
    }

    cleanup_sqlite_path(&path);
    Ok(())
}

#[tokio::test]
async fn git_agent_routes_proxy_status_requests_and_failures()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let stub = StubGitAgent::start().await?;
    let app = test_router(&stub.base_url)?;

    let (status, value) = get_json(&app, "/git-agent/status").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["status"], "ready");

    let (status, requests) = get_json(&app, "/git-agent/requests").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(requests[0]["request_id"], "request-one");

    let (status, request) = get_json(&app, "/git-agent/requests/request-one").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(request["request_id"], "request-one");
    assert_eq!(request["raw_patch"], "diff --git a/README.md b/README.md");
    assert_eq!(request["patch"]["files"][0]["path"], "README.md");

    let (status, rerun) = post_json(&app, "/git-agent/requests/request-one/rerun-review").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rerun["status"], "reviewing");
    assert_eq!(rerun["rerun"], true);

    let (status, missing) = get_json(&app, "/git-agent/requests/missing").await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["detail"], "git_agent returned HTTP 404 Not Found");

    Ok(())
}

#[tokio::test]
async fn git_agent_workspace_is_builtin_reserved_and_openable()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let agent_host = StubAgentHost::start().await?;
    let app = test_router_with_agent_host_and_stores(
        "http://127.0.0.1:9",
        &agent_host.base_url,
        StoreSet::in_memory(),
    )?;

    let (status, workspaces) = get_json(&app, "/workspaces").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workspaces[0]["workspace_id"], "git-agent");
    assert_eq!(workspaces[0]["name"], "GitAgent Repository");
    assert_eq!(workspaces[0]["builtin"], true);
    assert_eq!(workspaces[0]["volume_name"], "custom-git-agent-data");

    let (status, workspace) = get_json(&app, "/workspaces/git-agent").await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workspace, workspaces[0]);

    let (status, error) = request_json(
        &app,
        Method::POST,
        "/workspaces",
        Some(json!({ "workspace_id": "git-agent", "name": "Override" })),
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        error["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("built-in")
    );

    let (status, vscode) = request_json(
        &app,
        Method::POST,
        "/workspaces/git-agent/vscode",
        Some(json!({})),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(vscode["workspace_id"], "git-agent");
    assert_eq!(vscode["volume_name"], "custom-git-agent-data");
    assert_eq!(
        agent_host.recorded()?,
        vec![json!({
            "workspace_id": "git-agent",
            "volume_name": "custom-git-agent-data"
        })],
    );

    Ok(())
}

#[tokio::test]
async fn git_agent_workspace_mount_uses_git_agent_volume()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let agent_host = StubAgentHost::start().await?;
    let app = test_router_with_agent_host_and_stores(
        "http://127.0.0.1:9",
        &agent_host.base_url,
        StoreSet::in_memory(),
    )?;

    let (status, agent) = request_json(
        &app,
        Method::POST,
        "/agents",
        Some(json!({
            "agent_id": "workspace-agent",
            "name": "Workspace Agent",
            "workspace_mounts": [
                { "workspace_id": "git-agent", "mode": "ro" }
            ]
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent["workspace_mounts"][0]["workspace_id"], "git-agent");

    let (status, _session) = request_json(
        &app,
        Method::POST,
        "/sessions",
        Some(json!({ "agent_id": "workspace-agent" })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        agent_host.recorded()?,
        vec![json!({
            "harness": "acp",
            "skills": [],
            "workspace_mounts": [
                {
                    "workspace_id": "git-agent",
                    "mode": "ro",
                    "volume_name": "custom-git-agent-data"
                }
            ]
        })],
    );

    Ok(())
}

#[tokio::test]
async fn session_request_can_add_git_agent_workspace_mount()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let agent_host = StubAgentHost::start().await?;
    let app = test_router_with_agent_host_and_stores(
        "http://127.0.0.1:9",
        &agent_host.base_url,
        StoreSet::in_memory(),
    )?;
    let (status, _config) = get_json(&app, "/git-agent/config").await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _session) = request_json(
        &app,
        Method::POST,
        "/sessions",
        Some(json!({
            "agent_id": "git-agent",
            "workspace_mounts": [
                { "workspace_id": "git-agent", "mode": "rw" }
            ]
        })),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        agent_host.recorded()?,
        vec![json!({
            "env": {
                "KERNEL_SYSTEM_PROMPT": "Review submitted patches for correctness, safety, and repository policy before GitAgent commits them."
            },
            "harness": "acp",
            "skills": [],
            "workspace_mounts": [
                {
                    "workspace_id": "git-agent",
                    "mode": "rw",
                    "volume_name": "custom-git-agent-data"
                }
            ]
        })],
    );

    Ok(())
}
