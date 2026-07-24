#![allow(clippy::too_many_lines)]

//! Config-set bundle (zip) route tests backed by a stub `agent_host` so skill
//! staging during apply succeeds. Covers exact-source round-trip, canonical
//! inlining of relative skill sources, ZIP safety rejection, and that
//! `validate`/`plan` accept `application/zip` bodies exactly like `apply`.

use std::{
    collections::BTreeMap,
    error::Error,
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use client_service_rs::{AppConfig, AppState, agent_host::AgentHostClient, build_router};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;
use zip::write::SimpleFileOptions;

const BUNDLE_MANIFEST_NAME: &str = "agentspace-config.yaml";

const MANIFEST: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: skills/my-skill
";

#[derive(Clone, Default)]
struct StubState {
    requests: Arc<Mutex<Vec<(Method, String)>>>,
}

impl StubState {
    fn record(&self, method: Method, path: impl Into<String>) -> Result<(), StatusCode> {
        self.requests
            .lock()
            .map_err(|_error| StatusCode::INTERNAL_SERVER_ERROR)?
            .push((method, path.into()));
        Ok(())
    }
}

struct TestServer {
    base_url: String,
    handle: JoinHandle<Result<(), std::io::Error>>,
}

impl TestServer {
    async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let state = StubState::default();
        let app = stub_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let handle = tokio::spawn(async move { axum::serve(listener, app).await });
        Ok(Self {
            base_url: format!("http://{address}"),
            handle,
        })
    }

    fn app(&self) -> Result<Router, Box<dyn Error + Send + Sync>> {
        let config = AppConfig::new("127.0.0.1", 0, &self.base_url, BTreeMap::new());
        let agent_host = AgentHostClient::new(&self.base_url, Duration::from_secs(5))?;
        Ok(build_router(AppState::with_agent_host(config, agent_host)?))
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn stub_router(state: StubState) -> Router {
    Router::new()
        .route("/skills", post(create_host_skill).get(list_host_skills))
        .route(
            "/skills/{skill_id}",
            axum::routing::put(update_host_skill).delete(delete_host_skill),
        )
        .route(
            "/gateways",
            post(create_host_gateway).get(list_host_gateways),
        )
        .route("/gateways/{gateway_id}", delete(delete_host_gateway))
        .with_state(state)
}

async fn list_host_skills(State(state): State<StubState>) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, "/skills")?;
    Ok(Json(json!([])))
}

async fn create_host_skill(State(state): State<StubState>, Json(body): Json<Value>) -> Response {
    if let Err(status) = state.record(Method::POST, "/skills") {
        return status.into_response();
    }
    Json(json!({ "skill_id": body["skill_id"], "files": body["files"] })).into_response()
}

async fn update_host_skill(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::PUT, format!("/skills/{skill_id}"))?;
    Ok(Json(
        json!({ "skill_id": skill_id, "files": body["files"] }),
    ))
}

async fn delete_host_skill(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state.record(Method::DELETE, format!("/skills/{skill_id}"))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_host_gateways(State(state): State<StubState>) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, "/gateways")?;
    Ok(Json(json!([])))
}

async fn create_host_gateway(State(state): State<StubState>, Json(body): Json<Value>) -> Response {
    if let Err(status) = state.record(Method::POST, "/gateways") {
        return status.into_response();
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
    state.record(Method::DELETE, format!("/gateways/{gateway_id}"))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn send(
    app: &Router,
    request: Request<Body>,
) -> Result<(StatusCode, BTreeMap<String, String>, Vec<u8>), Box<dyn Error + Send + Sync>> {
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let body = to_bytes(response.into_body(), usize::MAX).await?.to_vec();
    Ok((status, headers, body))
}

fn build_zip(entries: &[(&str, &str)]) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = SimpleFileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options)?;
            writer.write_all(contents.as_bytes())?;
        }
        writer.finish()?;
    }
    Ok(buffer)
}

#[tokio::test]
async fn bundle_apply_source_export_is_byte_identical() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let server = TestServer::start().await?;
    let app = server.app()?;
    let bundle = build_zip(&[
        (BUNDLE_MANIFEST_NAME, MANIFEST),
        ("skills/my-skill/SKILL.md", "# hello"),
        ("skills/my-skill/scripts/run.sh", "echo hi"),
    ])?;

    let (status, _headers, _body) = send(
        &app,
        Request::post("/config/apply")
            .header(header::CONTENT_TYPE, "application/zip")
            .body(Body::from(bundle.clone()))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // Source export returns the exact uploaded zip bytes as application/zip
    // with a .zip download filename.
    let (status, headers, body) =
        send(&app, Request::get("/config/export").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, bundle, "source bundle export was not byte-identical");
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE.as_str())
            .map(String::as_str),
        Some("application/zip")
    );
    let disposition = headers
        .get(header::CONTENT_DISPOSITION.as_str())
        .map(String::as_str)
        .unwrap_or_default();
    assert!(
        disposition.contains(".zip\""),
        "expected a .zip download filename, got: {disposition}"
    );

    // Canonical export inlines the expanded skill files as YAML.
    let (status, _headers, canonical) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(canonical)?;
    assert!(
        text.contains("# hello") && text.contains("echo hi"),
        "canonical export did not inline bundle files: {text}"
    );
    Ok(())
}

#[tokio::test]
async fn bundle_apply_merges_multiple_yaml_with_relative_sources()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // A skill manifest under `config/` that resolves its source relative to its
    // own directory, plus a separate agent document. Skill-owned YAML inside the
    // source directory must be treated as content, never as a manifest.
    let skill_manifest = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: ../skills/my-skill
";
    let agent_manifest = r"apiVersion: agentspace.dev/v1alpha1
kind: Agent
metadata:
  name: helper
spec:
  name: Helper
  harness: acp
  systemPrompt: be helpful
  skills:
    - my-skill
";
    let bundle = build_zip(&[
        ("config/skill.yaml", skill_manifest),
        ("skills/my-skill/SKILL.md", "# relative hello"),
        ("skills/my-skill/extra.yaml", "note: skill-owned yaml"),
        ("config/agent.yml", agent_manifest),
    ])?;

    let (status, _headers, _body) = send(
        &app,
        Request::post("/config/apply")
            .header(header::CONTENT_TYPE, "application/zip")
            .body(Body::from(bundle.clone()))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // Exact source export round-trips byte-for-byte.
    let (status, _headers, body) =
        send(&app, Request::get("/config/export").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body, bundle,
        "multi-yaml bundle source was not byte-identical"
    );

    // Canonical export merges both documents and inlines the relative source,
    // including the skill-owned YAML file as content.
    let (status, _headers, canonical) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(canonical)?;
    assert!(
        text.contains("# relative hello"),
        "canonical export did not inline the relative skill source: {text}"
    );
    assert!(
        text.contains("skill-owned yaml"),
        "canonical export did not inline skill-owned YAML content: {text}"
    );
    assert!(
        text.contains("id: helper") && text.contains("name: Helper"),
        "canonical export did not include the merged agent document: {text}"
    );
    Ok(())
}

#[tokio::test]
async fn bundle_apply_rejects_path_traversal() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    let bundle = build_zip(&[
        (BUNDLE_MANIFEST_NAME, MANIFEST),
        ("../escape.txt", "malicious"),
    ])?;

    let (status, _headers, _body) = send(
        &app,
        Request::post("/config/apply")
            .header(header::CONTENT_TYPE, "application/zip")
            .body(Body::from(bundle))?,
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    Ok(())
}

#[tokio::test]
async fn bundle_validate_accepts_zip_body() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    let bundle = build_zip(&[
        (BUNDLE_MANIFEST_NAME, MANIFEST),
        ("skills/my-skill/SKILL.md", "# hello"),
    ])?;

    let (status, _headers, body) = send(
        &app,
        Request::post("/config/validate")
            .header(header::CONTENT_TYPE, "application/zip")
            .body(Body::from(bundle))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["valid"], json!(true), "bundle rejected: {value}");
    Ok(())
}

#[tokio::test]
async fn bundle_plan_accepts_zip_body_and_reports_generation()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;
    let bundle = build_zip(&[
        (BUNDLE_MANIFEST_NAME, MANIFEST),
        ("skills/my-skill/SKILL.md", "# hello"),
    ])?;

    let (status, _headers, body) = send(
        &app,
        Request::post("/config/plan")
            .header(header::CONTENT_TYPE, "application/zip")
            .body(Body::from(bundle))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert!(
        value.get("active_generation").is_some(),
        "plan did not report active_generation: {value}"
    );
    let entries = value["plan"]["changes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        entries
            .iter()
            .any(|entry| entry["kind"] == "skill" && entry["id"] == "my-skill"),
        "plan did not include the bundled skill: {value}"
    );
    Ok(())
}

#[tokio::test]
async fn config_route_accepts_bundle_larger_than_default_body_limit()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // Build a bundle whose compressed body exceeds the framework's 2 MiB
    // default limit (stored, i.e. uncompressed, so the size is predictable) but
    // stays under the per-entry/total bundle limits. It must be accepted because
    // the config routes raise the body limit above the declared bundle size.
    let large_file = "a".repeat(3 * 1024 * 1024);
    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let stored =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file(BUNDLE_MANIFEST_NAME, SimpleFileOptions::default())?;
        writer.write_all(MANIFEST.as_bytes())?;
        writer.start_file("skills/my-skill/SKILL.md", SimpleFileOptions::default())?;
        writer.write_all(b"# hello")?;
        writer.start_file("skills/my-skill/big.txt", stored)?;
        writer.write_all(large_file.as_bytes())?;
        writer.finish()?;
    }
    assert!(
        buffer.len() > 2 * 1024 * 1024,
        "test bundle must exceed the default 2 MiB body limit, got {} bytes",
        buffer.len()
    );

    let (status, _headers, body) = send(
        &app,
        Request::post("/config/validate")
            .header(header::CONTENT_TYPE, "application/zip")
            .body(Body::from(buffer))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "large valid bundle was rejected");
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["valid"], json!(true), "bundle rejected: {value}");
    Ok(())
}

#[tokio::test]
async fn config_route_rejects_body_over_configured_limit()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // A request body above the configured 40 MiB config body limit must be
    // rejected before any bundle parsing occurs.
    let oversized = vec![0_u8; 41 * 1024 * 1024];
    let (status, _headers, _body) = send(
        &app,
        Request::post("/config/apply")
            .header(header::CONTENT_TYPE, "application/zip")
            .body(Body::from(oversized))?,
    )
    .await?;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    Ok(())
}
