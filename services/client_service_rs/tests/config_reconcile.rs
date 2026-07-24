#![allow(clippy::too_many_lines)]

//! Reconciliation and skill-route consistency tests backed by a stub
//! `agent_host`. These prove that (1) validate/plan/apply accept installed
//! builtin skill references, (2) apply reconciles user skills and gateways
//! against `agent_host`, and (3) failed skill-route staging never commits the
//! config document.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
use client_service_rs::{
    AppConfig, AppState,
    agent_host::AgentHostClient,
    api::{reconcile_gateways_on_startup, reconcile_skills_on_startup},
    build_router,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct StubState {
    requests: Arc<Mutex<Vec<(Method, String)>>>,
    // When set, `GET /skills` fails so the upstream user-skill state cannot be
    // determined (used to prove apply/reconcile refuse to proceed).
    fail_list_skills: Arc<AtomicBool>,
    // Gateway ids whose FIRST `DELETE /gateways/{id}` fails (then succeeds), used
    // to prove startup orphan destruction retries transient failures.
    destroy_fail_once: Arc<Mutex<BTreeSet<String>>>,
}

impl StubState {
    fn record(&self, method: Method, path: impl Into<String>) -> Result<(), StatusCode> {
        self.requests
            .lock()
            .map_err(|_error| StatusCode::INTERNAL_SERVER_ERROR)?
            .push((method, path.into()));
        Ok(())
    }

    fn recorded(&self) -> Result<Vec<(Method, String)>, Box<dyn Error + Send + Sync>> {
        Ok(self
            .requests
            .lock()
            .map_err(|_error| "stub request mutex poisoned")?
            .clone())
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
            base_url: format!("http://{address}"),
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
        AppState::with_agent_host(config, agent_host)
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
            axum::routing::get(get_host_skill)
                .put(update_host_skill)
                .delete(delete_host_skill),
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
    if state.fail_list_skills.load(Ordering::SeqCst) {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    // Realistic production summaries omit file contents; details are fetched
    // via GET /skills/{id}.
    Ok(Json(json!([
        { "skill_id": "builtin-skill", "source": "builtin" },
        { "skill_id": "stale-skill", "source": "user" }
    ])))
}

async fn get_host_skill(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, format!("/skills/{skill_id}"))?;
    let files = match skill_id.as_str() {
        "stale-skill" => json!({ "SKILL.md": "old" }),
        "builtin-skill" => json!({ "SKILL.md": "# builtin" }),
        _ => return Err(StatusCode::NOT_FOUND),
    };
    Ok(Json(json!({
        "skill_id": skill_id,
        "source": if skill_id == "builtin-skill" { "builtin" } else { "user" },
        "files": files,
    })))
}

async fn create_host_skill(State(state): State<StubState>, Json(body): Json<Value>) -> Response {
    if let Err(status) = state.record(Method::POST, "/skills") {
        return status.into_response();
    }
    if body["skill_id"] == "fail-skill" {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": "upstream skill create failed" })),
        )
            .into_response();
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
    Ok(Json(json!([
        { "gateway_id": "stale-gw", "container_name": "stale" }
    ])))
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
    let fail = state
        .destroy_fail_once
        .lock()
        .map_err(|_error| StatusCode::INTERNAL_SERVER_ERROR)?
        .remove(&gateway_id);
    if fail {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[tokio::test]
async fn concurrent_skill_creates_are_serialized_without_lost_updates()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // Concurrent create_skill calls each read-modify-write the ConfigDocument.
    // The shared apply/reconcile lock must serialize them so no update is lost
    // and the document never diverges from agent_host (no split-brain).
    let ids: Vec<String> = (0..8)
        .map(|index| format!("concurrent-skill-{index}"))
        .collect();
    let mut handles = Vec::new();
    for id in &ids {
        let app = app.clone();
        let id = id.clone();
        handles.push(tokio::spawn(async move {
            let request = Request::post("/skills")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({ "skill_id": id, "files": { "SKILL.md": "# concurrent" } }).to_string(),
                ))?;
            let response = app.oneshot(request).await?;
            Ok::<StatusCode, Box<dyn Error + Send + Sync>>(response.status())
        }));
    }
    for handle in handles {
        let status = handle.await??;
        assert_eq!(status, StatusCode::OK, "concurrent create failed: {status}");
    }

    // Every concurrently-created skill must be present in the committed document.
    let (status, body) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    for id in &ids {
        assert!(
            text.contains(id),
            "lost update: {id} missing from document:\n{text}"
        );
    }
    Ok(())
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

fn yaml_post(path: &str, source: &str) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::post(path)
        .header(header::CONTENT_TYPE, "application/yaml")
        .body(Body::from(source.to_owned()))?)
}

#[tokio::test]
async fn validate_accepts_builtin_skill_reference() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    let valid = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: be helpful
      skills:
        - builtin-skill
";
    let (status, body) = send(&app, yaml_post("/config/validate", valid)?).await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["valid"], json!(true), "builtin ref rejected: {value}");

    // A typo in the skill reference is not a builtin and must be rejected.
    let typo = valid.replace("builtin-skill", "builtin-skil");
    let (status, body) = send(&app, yaml_post("/config/validate", &typo)?).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let value: Value = serde_json::from_slice(&body)?;
    let issues = value["issues"].as_array().cloned().unwrap_or_default();
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "unresolved_skill_reference"),
        "expected unresolved_skill_reference, got: {value}"
    );
    Ok(())
}

#[tokio::test]
async fn apply_reconciles_skills_and_gateways() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: my-skill
      files:
        SKILL.md: '# my skill'
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: be helpful
  gateways:
    - id: echo-gw
      name: Echo
      type: echo
      agent: helper
      enabled: true
";
    let (status, body) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    let reconciliation = &value["reconciliation"];
    assert_eq!(
        reconciliation["ok"],
        json!(true),
        "reconcile reported failures: {value}"
    );

    let recorded = server.state.recorded()?;
    assert!(
        recorded.contains(&(Method::POST, "/skills".to_owned())),
        "expected user skill to be created upstream: {recorded:?}"
    );
    assert!(
        recorded.contains(&(Method::DELETE, "/skills/stale-skill".to_owned())),
        "expected stale user skill to be removed: {recorded:?}"
    );
    assert!(
        recorded.contains(&(Method::POST, "/gateways".to_owned())),
        "expected enabled gateway to be started: {recorded:?}"
    );
    assert!(
        recorded.contains(&(Method::DELETE, "/gateways/stale-gw".to_owned())),
        "expected removed gateway to be destroyed: {recorded:?}"
    );
    // The list endpoint omits file contents, so per-skill details must be
    // fetched before comparison/removal (never treated as empty).
    assert!(
        recorded.contains(&(Method::GET, "/skills/stale-skill".to_owned())),
        "expected per-skill details to be fetched before removal: {recorded:?}"
    );
    Ok(())
}

#[tokio::test]
async fn apply_uses_fetched_details_and_skips_unchanged_skill()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // The document declares stale-skill with the SAME files the stub serves via
    // GET /skills/{id}. If reconciliation treated the file-less list summary as
    // an empty skill it would spuriously rewrite it; instead it must fetch the
    // details, compare, and skip.
    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: stale-skill
      files:
        SKILL.md: old
";
    let (status, body) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["reconciliation"]["ok"], json!(true), "body: {value}");

    let recorded = server.state.recorded()?;
    assert!(
        recorded.contains(&(Method::GET, "/skills/stale-skill".to_owned())),
        "expected per-skill details to be fetched: {recorded:?}"
    );
    assert!(
        !recorded
            .iter()
            .any(|(method, path)| *method == Method::PUT && path == "/skills/stale-skill"),
        "unchanged skill must not be rewritten: {recorded:?}"
    );
    assert!(
        !recorded
            .iter()
            .any(|(method, path)| *method == Method::DELETE && path.starts_with("/skills/")),
        "declared skill must not be removed: {recorded:?}"
    );
    Ok(())
}

#[tokio::test]
async fn apply_compensates_staged_skills_on_failure() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // alpha-skill sorts/stages before fail-skill: alpha is created upstream, then
    // fail-skill's create fails, so alpha must be compensated (deleted) and the
    // document must never commit.
    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: alpha-skill
      files:
        SKILL.md: '# alpha'
    - id: fail-skill
      files:
        SKILL.md: '# fail'
";
    let (status, _body) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert!(
        status.is_server_error() || status.is_client_error(),
        "expected apply to abort, got {status}"
    );

    let recorded = server.state.recorded()?;
    assert!(
        recorded.contains(&(Method::DELETE, "/skills/alpha-skill".to_owned())),
        "expected staged alpha-skill to be compensated (deleted): {recorded:?}"
    );

    // The failed apply must not have committed either skill.
    let (status, body) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert!(
        !text.contains("alpha-skill") && !text.contains("fail-skill"),
        "aborted apply leaked skills into the committed document: {text}"
    );
    Ok(())
}

#[tokio::test]
async fn skill_create_failure_does_not_commit_document() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let server = TestServer::start().await?;
    let app = server.app()?;

    let (status, _body) = send(
        &app,
        Request::post("/skills")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&json!({
                "skill_id": "fail-skill",
                "files": { "SKILL.md": "# fail" }
            }))?))?,
    )
    .await?;
    assert!(
        status.is_server_error() || status.is_client_error(),
        "expected an error status, got {status}"
    );

    // The document must not have been committed with the failed skill.
    let (status, body) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert!(
        !text.contains("fail-skill"),
        "failed skill leaked into the committed document: {text}"
    );
    Ok(())
}

const AGENT_ONLY_SOURCE: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: be helpful
";

#[tokio::test]
async fn startup_reconcile_starts_enabled_and_destroys_orphan_gateways()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let state = server.app_state()?;
    let app = build_router(state.clone());

    // Desired: one enabled gateway (echo-gw). Observed upstream (fixed stub):
    // only stale-gw, which is an orphan.
    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: be helpful
  gateways:
    - id: echo-gw
      name: Echo
      type: echo
      agent: helper
      enabled: true
";
    let (status, _body) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert_eq!(status, StatusCode::OK);

    // Run the startup reconcile independently of apply and assert it drives the
    // complete desired-vs-observed reconcile.
    server.state.clear_recorded()?;
    reconcile_gateways_on_startup(state.clone()).await;

    let recorded = server.state.recorded()?;
    assert!(
        recorded.contains(&(Method::POST, "/gateways".to_owned())),
        "startup reconcile did not start the enabled gateway: {recorded:?}"
    );
    assert!(
        recorded.contains(&(Method::DELETE, "/gateways/stale-gw".to_owned())),
        "startup reconcile did not destroy the orphan gateway: {recorded:?}"
    );
    Ok(())
}

#[tokio::test]
async fn startup_reconcile_stops_disabled_but_running_gateway()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let state = server.app_state()?;
    let app = build_router(state.clone());

    // Desired: a disabled gateway whose id matches the running upstream gateway
    // (stale-gw). The startup reconcile must stop it.
    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: be helpful
  gateways:
    - id: stale-gw
      name: Stale
      type: echo
      agent: helper
      enabled: false
";
    let (status, _body) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert_eq!(status, StatusCode::OK);

    server.state.clear_recorded()?;
    reconcile_gateways_on_startup(state.clone()).await;

    let recorded = server.state.recorded()?;
    assert!(
        recorded.contains(&(Method::DELETE, "/gateways/stale-gw".to_owned())),
        "startup reconcile did not stop the disabled-but-running gateway: {recorded:?}"
    );
    assert!(
        !recorded.contains(&(Method::POST, "/gateways".to_owned())),
        "disabled gateway must not be started: {recorded:?}"
    );
    Ok(())
}

#[tokio::test]
async fn startup_reconcile_retries_orphan_gateway_destroy()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let state = server.app_state()?;
    let app = build_router(state.clone());

    // Desired: no gateways. Observed: stale-gw (orphan). Its first destroy fails
    // and must be retried until it succeeds.
    let (status, _body) = send(&app, yaml_post("/config/apply", AGENT_ONLY_SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    server.state.clear_recorded()?;
    server
        .state
        .destroy_fail_once
        .lock()
        .map_err(|_error| "stub mutex poisoned")?
        .insert("stale-gw".to_owned());

    reconcile_gateways_on_startup(state.clone()).await;

    let recorded = server.state.recorded()?;
    let destroy_attempts = recorded
        .iter()
        .filter(|(method, path)| *method == Method::DELETE && path == "/gateways/stale-gw")
        .count();
    assert!(
        destroy_attempts >= 2,
        "expected the orphan destroy to be retried after a transient failure, got \
         {destroy_attempts} attempts: {recorded:?}"
    );
    Ok(())
}

#[tokio::test]
async fn startup_reconcile_materializes_and_removes_skills()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let state = server.app_state()?;
    let app = build_router(state.clone());

    // Desired: my-skill. Observed user skill (fixed stub): stale-skill (orphan).
    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: my-skill
      files:
        SKILL.md: '# my skill'
";
    let (status, _body) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert_eq!(status, StatusCode::OK);

    server.state.clear_recorded()?;
    reconcile_skills_on_startup(state.clone()).await;

    let recorded = server.state.recorded()?;
    assert!(
        recorded.contains(&(Method::POST, "/skills".to_owned())),
        "startup skill reconcile did not create the desired skill: {recorded:?}"
    );
    assert!(
        recorded.contains(&(Method::DELETE, "/skills/stale-skill".to_owned())),
        "startup skill reconcile did not remove the orphan skill: {recorded:?}"
    );
    // Per-skill details must be fetched, never treated as empty.
    assert!(
        recorded.contains(&(Method::GET, "/skills/stale-skill".to_owned())),
        "startup skill reconcile did not fetch skill details: {recorded:?}"
    );
    Ok(())
}

#[tokio::test]
async fn apply_fails_when_upstream_skill_state_is_unknown()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // The upstream user-skill listing fails, so the state cannot be determined.
    // Even though the desired document declares no skills, apply must NOT report
    // success or commit: it cannot confirm nothing needs staging/removing.
    server.state.fail_list_skills.store(true, Ordering::SeqCst);

    let (status, _body) = send(&app, yaml_post("/config/apply", AGENT_ONLY_SOURCE)?).await?;
    assert!(
        status.is_server_error() || status.is_client_error(),
        "apply must fail when the upstream skill state is unknown, got {status}"
    );

    // Allow the listing to succeed again and confirm the failed apply did not
    // commit the document.
    server.state.fail_list_skills.store(false, Ordering::SeqCst);
    let (status, body) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert!(
        !text.contains("helper"),
        "aborted apply leaked the agent into the committed document: {text}"
    );
    Ok(())
}
