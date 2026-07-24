//! End-to-end coverage that the resolved Git Agent discovery URLs reach the
//! runtime agent session environment sent to `agent_host`, and that a secret
//! rotation takes effect on the next session (no restart).

use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode, header},
    routing::{get, post},
};
use client_service_rs::{AppConfig, AppState, agent_host::AgentHostClient, build_router};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

const REMOTE_URL_V1: &str = "https://git.example.com/secret-repo.git";
const REMOTE_URL_V2: &str = "https://git.example.com/rotated-repo.git";

const SOURCE: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  secrets:
    - name: GIT_REMOTE_URL
      description: Remote URL
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: hello
  gitAgent:
    enabled: true
    defaultBranch: trunk
    remoteUrl:
      secretRef: GIT_REMOTE_URL
    patchUrl: http://git/PatchRequest
    reviewAgent: git-agent
";

/// Captures the `env` map from every `POST /sessions` body the stub receives.
#[derive(Clone, Default)]
struct Captured {
    session_envs: Arc<Mutex<Vec<Value>>>,
}

fn build_app_with_capture() -> Result<(Router, Captured), Box<dyn Error + Send + Sync>> {
    let captured = Captured::default();
    let base_url = spawn_stub_agent_host(captured.clone())?;
    let config = AppConfig::new("127.0.0.1", 0, &base_url, BTreeMap::new());
    let agent_host = AgentHostClient::new(&base_url, Duration::from_secs(5))?;
    Ok((
        build_router(AppState::with_agent_host(config, agent_host)?),
        captured,
    ))
}

fn spawn_stub_agent_host(captured: Captured) -> Result<String, Box<dyn Error + Send + Sync>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let app = Router::new()
        .route("/skills", get(stub_empty_list).post(stub_ok_json))
        .route("/gateways", get(stub_empty_list))
        .route("/sessions", post(capture_session))
        .with_state(captured);
    tokio::spawn(async move {
        let listener = TcpListener::from_std(listener)?;
        axum::serve(listener, app).await
    });
    Ok(format!("http://{address}"))
}

async fn stub_empty_list() -> Json<Value> {
    Json(json!([]))
}

async fn stub_ok_json(Json(body): Json<Value>) -> Json<Value> {
    Json(body)
}

async fn capture_session(
    State(captured): State<Captured>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    let env = body
        .get("env")
        .cloned()
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    captured
        .session_envs
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .push(env);
    Ok(Json(json!({
        "session_id": "host-session",
        "status": "running"
    })))
}

async fn send(
    app: &Router,
    request: Request<Body>,
) -> Result<(StatusCode, Vec<u8>), Box<dyn Error + Send + Sync>> {
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await?.to_vec();
    Ok((status, body))
}

fn apply_request(source: &str) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::post("/config/apply")
        .header(header::CONTENT_TYPE, "application/yaml")
        .body(Body::from(source.to_owned()))?)
}

fn set_secret_request(value: &str) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::put("/secrets/GIT_REMOTE_URL/value")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "value": value }).to_string()))?)
}

fn create_session_request() -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::post("/sessions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "agent_id": "helper",
                "channel_name": "web",
                "client_type": "webui"
            })
            .to_string(),
        ))?)
}

fn last_env(captured: &Captured) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let envs = captured
        .session_envs
        .lock()
        .map_err(|_| "session env mutex poisoned")?;
    envs.last()
        .cloned()
        .ok_or_else(|| "no session recorded".into())
}

#[tokio::test]
async fn session_env_carries_resolved_git_agent_urls_and_rotates()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let (app, captured) = build_app_with_capture()?;

    let (status, body) = send(&app, apply_request(SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK, "apply failed: {body:?}");

    // Before the secret is set, session creation must fail closed with an
    // actionable missing-secret error that names the Git Agent field.
    let (status, body) = send(&app, create_session_request()?).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let parsed: Value = serde_json::from_slice(&body)?;
    assert_eq!(parsed["error"], "secret_values_unset");
    assert!(
        parsed["missing_secrets"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| item["field"] == "gitAgent/remoteUrl")),
        "missing secret should name gitAgent/remoteUrl: {parsed}"
    );

    // Set the secret; the resolved URL must now reach the session env.
    let (status, _body) = send(&app, set_secret_request(REMOTE_URL_V1)?).await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _body) = send(&app, create_session_request()?).await?;
    assert_eq!(status, StatusCode::OK);
    let env = last_env(&captured)?;
    assert_eq!(env["GITAGENT_REMOTE_URL"], REMOTE_URL_V1);
    assert_eq!(env["GITAGENT_PATCH_URL"], "http://git/PatchRequest");
    assert_eq!(env["GITAGENT_DEFAULT_BRANCH"], "trunk");

    // Rotate the secret; the next session must observe the new value without a
    // restart.
    let (status, _body) = send(&app, set_secret_request(REMOTE_URL_V2)?).await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _body) = send(&app, create_session_request()?).await?;
    assert_eq!(status, StatusCode::OK);
    let env = last_env(&captured)?;
    assert_eq!(env["GITAGENT_REMOTE_URL"], REMOTE_URL_V2);

    Ok(())
}
