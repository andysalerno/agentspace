#![allow(clippy::too_many_lines)]

use std::{collections::BTreeMap, error::Error, time::Duration};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::Path,
    http::{Request, StatusCode, header},
    routing::{delete, get},
};
use client_service_rs::{AppConfig, AppState, agent_host::AgentHostClient, build_router};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

const INTERNAL_TOKEN: &str = "internal-secret-token";
const REMOTE_URL_VALUE: &str = "https://git.example.com/secret-repo.git";

const SOURCE: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  secrets:
    - name: GIT_REMOTE_URL
      description: Remote URL
  gitAgent:
    enabled: true
    defaultBranch: trunk
    allowedRefPrefixes:
      - feature/
    remoteUrl:
      secretRef: GIT_REMOTE_URL
    patchUrl: http://git/PatchRequest
    reviewAgent: git-agent
    validationCommand: just validate
";

fn router_with_token(token: Option<&str>) -> Result<Router, Box<dyn Error + Send + Sync>> {
    let mut env = BTreeMap::new();
    if let Some(token) = token {
        env.insert("CLIENT_SERVICE_INTERNAL_TOKEN".to_owned(), token.to_owned());
    }
    // Apply requires a reachable agent_host to determine upstream user-skill
    // state, so stand up a minimal stub reporting empty state.
    let base_url = spawn_stub_agent_host()?;
    let config = AppConfig::new("127.0.0.1", 0, &base_url, env);
    let agent_host = AgentHostClient::new(&base_url, Duration::from_secs(5))?;
    Ok(build_router(AppState::with_agent_host(config, agent_host)?))
}

/// Spawn a minimal in-process stub `agent_host` for apply reconciliation and
/// return its base URL. The server runs for the lifetime of the test process.
fn spawn_stub_agent_host() -> Result<String, Box<dyn Error + Send + Sync>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let app = Router::new()
        .route("/skills", get(stub_empty_list).post(stub_ok_json))
        .route(
            "/skills/{skill_id}",
            get(stub_get_skill)
                .put(stub_ok_json)
                .delete(stub_no_content),
        )
        .route("/gateways", get(stub_empty_list).post(stub_create_gateway))
        .route("/gateways/{gateway_id}", delete(stub_no_content));
    tokio::spawn(async move {
        let listener = TcpListener::from_std(listener)?;
        axum::serve(listener, app).await
    });
    Ok(format!("http://{address}"))
}

async fn stub_empty_list() -> Json<Value> {
    Json(json!([]))
}

async fn stub_get_skill(Path(skill_id): Path<String>) -> Json<Value> {
    Json(json!({ "skill_id": skill_id, "source": "user", "files": {} }))
}

async fn stub_create_gateway(Json(body): Json<Value>) -> Json<Value> {
    Json(json!({
        "gateway_id": body["gateway_id"],
        "container_name": "gateway-container"
    }))
}

async fn stub_ok_json(Json(body): Json<Value>) -> Json<Value> {
    Json(body)
}

async fn stub_no_content() -> StatusCode {
    StatusCode::NO_CONTENT
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

fn effective_request(token: Option<&str>) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    let mut builder = Request::get("/internal/git-agent/effective-config");
    if let Some(token) = token {
        builder = builder.header("x-internal-token", token);
    }
    Ok(builder.body(Body::empty())?)
}

#[tokio::test]
async fn effective_config_disabled_without_internal_token()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router_with_token(None)?;
    let (status, _body) = send(&app, apply_request(SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _body) = send(&app, effective_request(Some("anything"))?).await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn effective_config_rejects_missing_or_wrong_token()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router_with_token(Some(INTERNAL_TOKEN))?;
    let (status, _body) = send(&app, apply_request(SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _body) = send(&app, effective_request(None)?).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _body) = send(&app, effective_request(Some("wrong-token"))?).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn effective_config_reports_missing_secret_actionably()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router_with_token(Some(INTERNAL_TOKEN))?;
    let (status, _body) = send(&app, apply_request(SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    // Secret is declared but not set: resolution must fail with an actionable
    // 409 listing the missing secret and its field path, never a value.
    let (status, body) = send(&app, effective_request(Some(INTERNAL_TOKEN))?).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let parsed: Value = serde_json::from_slice(&body)?;
    assert_eq!(parsed["error"], "secret_values_unset");
    let missing = parsed["missing_secrets"]
        .as_array()
        .ok_or("missing_secrets should be an array")?;
    assert!(
        missing.iter().any(|item| {
            item["name"] == "GIT_REMOTE_URL" && item["field"] == "gitAgent/remoteUrl"
        })
    );
    assert!(!String::from_utf8_lossy(&body).contains(REMOTE_URL_VALUE));
    Ok(())
}

#[tokio::test]
async fn effective_config_resolves_secrets_with_token() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let app = router_with_token(Some(INTERNAL_TOKEN))?;
    let (status, _body) = send(&app, apply_request(SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    // Set the declared secret value (write-only store).
    let (status, _body) = send(
        &app,
        Request::put("/secrets/GIT_REMOTE_URL/value")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "value": REMOTE_URL_VALUE }).to_string()))?,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = send(&app, effective_request(Some(INTERNAL_TOKEN))?).await?;
    assert_eq!(status, StatusCode::OK);
    let parsed: Value = serde_json::from_slice(&body)?;
    assert_eq!(parsed["configured"], true);
    assert_eq!(parsed["enabled"], true);
    assert_eq!(parsed["defaultBranch"], "trunk");
    assert_eq!(parsed["allowedRefPrefixes"][0], "feature/");
    assert_eq!(parsed["remoteUrl"], REMOTE_URL_VALUE);
    assert_eq!(parsed["patchUrl"], "http://git/PatchRequest");
    assert_eq!(parsed["reviewAgent"], "git-agent");
    assert_eq!(parsed["validationCommand"], "just validate");

    // The resolved secret value must never appear in the config export.
    let (status, source) = send(&app, Request::get("/config/export").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(!String::from_utf8_lossy(&source).contains(REMOTE_URL_VALUE));
    Ok(())
}

#[tokio::test]
async fn effective_config_get_does_not_mutate_source() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router_with_token(Some(INTERNAL_TOKEN))?;
    let (status, _body) = send(&app, apply_request(SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    let (status, before) = send(&app, Request::get("/config/export").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);

    // Resolution fails (secret unset) but must not mutate authored source.
    let (status, _body) = send(&app, effective_request(Some(INTERNAL_TOKEN))?).await?;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, after) = send(&app, Request::get("/config/export").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before, after);
    Ok(())
}
