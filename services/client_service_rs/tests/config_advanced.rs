#![allow(clippy::too_many_lines)]

//! Advanced declarative-config tests backed by a *stateful* stub `agent_host`
//! that tracks skills and gateways as real state. These cover generation CAS
//! (optimistic concurrency), no-op apply stability, skill-version rollback
//! document sync, referential integrity, `ConfigDocument` authority for skills,
//! and Git Agent secret-reference preservation across unrelated updates.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use client_service_rs::{AppConfig, AppState, agent_host::AgentHostClient, build_router};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;

type SkillFiles = BTreeMap<String, String>;

#[derive(Clone, Default)]
struct StubState {
    requests: Arc<Mutex<Vec<(Method, String)>>>,
    /// skill id -> (source, files)
    skills: Arc<Mutex<BTreeMap<String, (String, SkillFiles)>>>,
    /// gateway id -> present/running
    gateways: Arc<Mutex<BTreeSet<String>>>,
    /// skill id -> files returned when a version is rolled back
    rollback_files: Arc<Mutex<BTreeMap<String, SkillFiles>>>,
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

    fn clear_requests(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.requests
            .lock()
            .map_err(|_error| "stub request mutex poisoned")?
            .clear();
        Ok(())
    }

    fn seed_skill(&self, id: &str, source: &str, files: SkillFiles) {
        if let Ok(mut skills) = self.skills.lock() {
            skills.insert(id.to_owned(), (source.to_owned(), files));
        }
    }

    fn seed_rollback(&self, id: &str, files: SkillFiles) {
        if let Ok(mut map) = self.rollback_files.lock() {
            map.insert(id.to_owned(), files);
        }
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
            get(get_host_skill)
                .put(update_host_skill)
                .delete(delete_host_skill),
        )
        .route("/skills/{skill_id}/versions", get(list_host_versions))
        .route(
            "/skills/{skill_id}/versions/{version}/rollback",
            post(rollback_host_version),
        )
        .route(
            "/gateways",
            post(create_host_gateway).get(list_host_gateways),
        )
        .route(
            "/gateways/{gateway_id}",
            axum::routing::delete(delete_host_gateway),
        )
        .with_state(state)
}

fn skill_json(id: &str, source: &str, files: &SkillFiles) -> Value {
    json!({ "skill_id": id, "source": source, "files": files })
}

async fn list_host_skills(State(state): State<StubState>) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, "/skills")?;
    let list: Vec<Value> = {
        let skills = state
            .skills
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        skills
            .iter()
            .map(|(id, (source, files))| skill_json(id, source, files))
            .collect()
    };
    Ok(Json(Value::Array(list)))
}

async fn get_host_skill(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, format!("/skills/{skill_id}"))?;
    let skills = state
        .skills
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    skills
        .get(&skill_id)
        .map(|(source, files)| Json(skill_json(&skill_id, source, files)))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn create_host_skill(State(state): State<StubState>, Json(body): Json<Value>) -> Response {
    if let Err(status) = state.record(Method::POST, "/skills") {
        return status.into_response();
    }
    let id = body["skill_id"].as_str().unwrap_or_default().to_owned();
    let files: SkillFiles = serde_json::from_value(body["files"].clone()).unwrap_or_default();
    if let Ok(mut skills) = state.skills.lock() {
        skills.insert(id.clone(), ("user".to_owned(), files.clone()));
    }
    Json(skill_json(&id, "user", &files)).into_response()
}

async fn update_host_skill(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::PUT, format!("/skills/{skill_id}"))?;
    let files: SkillFiles = serde_json::from_value(body["files"].clone()).unwrap_or_default();
    if let Ok(mut skills) = state.skills.lock() {
        skills.insert(skill_id.clone(), ("user".to_owned(), files.clone()));
    }
    Ok(Json(skill_json(&skill_id, "user", &files)))
}

async fn delete_host_skill(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state.record(Method::DELETE, format!("/skills/{skill_id}"))?;
    if let Ok(mut skills) = state.skills.lock() {
        skills.remove(&skill_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_host_versions(
    State(state): State<StubState>,
    Path(skill_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, format!("/skills/{skill_id}/versions"))?;
    Ok(Json(json!([{ "version": 1 }, { "version": 2 }])))
}

async fn rollback_host_version(
    State(state): State<StubState>,
    Path((skill_id, version)): Path<(String, u64)>,
) -> Result<Json<Value>, StatusCode> {
    state.record(
        Method::POST,
        format!("/skills/{skill_id}/versions/{version}/rollback"),
    )?;
    let files = state
        .rollback_files
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .get(&skill_id)
        .cloned()
        .unwrap_or_default();
    if let Ok(mut skills) = state.skills.lock() {
        skills.insert(skill_id.clone(), ("user".to_owned(), files.clone()));
    }
    Ok(Json(skill_json(&skill_id, "user", &files)))
}

async fn list_host_gateways(State(state): State<StubState>) -> Result<Json<Value>, StatusCode> {
    state.record(Method::GET, "/gateways")?;
    let list: Vec<Value> = {
        let gateways = state
            .gateways
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        gateways
            .iter()
            .map(|id| json!({ "gateway_id": id, "container_name": format!("{id}-container") }))
            .collect()
    };
    Ok(Json(Value::Array(list)))
}

async fn create_host_gateway(State(state): State<StubState>, Json(body): Json<Value>) -> Response {
    if let Err(status) = state.record(Method::POST, "/gateways") {
        return status.into_response();
    }
    let id = body["gateway_id"].as_str().unwrap_or_default().to_owned();
    if let Ok(mut gateways) = state.gateways.lock() {
        gateways.insert(id.clone());
    }
    Json(json!({ "gateway_id": id, "container_name": format!("{id}-container") })).into_response()
}

async fn delete_host_gateway(
    State(state): State<StubState>,
    Path(gateway_id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state.record(Method::DELETE, format!("/gateways/{gateway_id}"))?;
    if let Ok(mut gateways) = state.gateways.lock() {
        gateways.remove(&gateway_id);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn send(
    app: &Router,
    request: Request<Body>,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Ok((status, value))
}

async fn send_bytes(
    app: &Router,
    request: Request<Body>,
) -> Result<(StatusCode, Vec<u8>), Box<dyn Error + Send + Sync>> {
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await?.to_vec();
    Ok((status, bytes))
}

fn yaml_post(path: &str, source: &str) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::post(path)
        .header(header::CONTENT_TYPE, "application/yaml")
        .body(Body::from(source.to_owned()))?)
}

fn yaml_apply_if_match(
    source: &str,
    generation: i64,
) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::post("/config/apply")
        .header(header::CONTENT_TYPE, "application/yaml")
        .header(header::IF_MATCH, generation.to_string())
        .body(Body::from(source.to_owned()))?)
}

fn json_post(path: &str, body: &Value) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body)?))?)
}

fn json_put(path: &str, body: &Value) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::put(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body)?))?)
}

fn agent_only_doc(prompt: &str) -> String {
    format!(
        "apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: {prompt}
"
    )
}

// ----- Item C: generation CAS -----

#[tokio::test]
async fn apply_enforces_generation_cas() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // Establish an initial generation.
    let (status, value) = send(&app, yaml_post("/config/apply", &agent_only_doc("v1"))?).await?;
    assert_eq!(status, StatusCode::OK, "initial apply failed: {value}");
    let gen1 = value["generation"]
        .as_i64()
        .ok_or("apply did not return a numeric generation")?;

    // Plan the next change: it must report the active generation for CAS.
    let (status, value) = send(&app, yaml_post("/config/plan", &agent_only_doc("v2"))?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value["active_generation"].as_i64(),
        Some(gen1),
        "plan did not surface the active generation: {value}"
    );

    // Applying with the correct expected generation succeeds and advances it.
    let (status, value) = send(&app, yaml_apply_if_match(&agent_only_doc("v2"), gen1)?).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "matched-generation apply failed: {value}"
    );
    let gen2 = value["generation"]
        .as_i64()
        .ok_or("no generation on apply")?;
    assert!(gen2 > gen1, "generation did not advance: {gen1} -> {gen2}");

    // A stale expected generation must be atomically rejected with 409.
    let (status, _value) = send(&app, yaml_apply_if_match(&agent_only_doc("v3"), gen1)?).await?;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "stale-generation apply was not rejected"
    );

    // The rejected apply must not have advanced the generation.
    let (status, value) = send(&app, yaml_post("/config/plan", &agent_only_doc("v3"))?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["active_generation"].as_i64(), Some(gen2));
    Ok(())
}

// ----- Item D: no-op apply stability -----

#[tokio::test]
async fn identical_apply_does_not_rewrite_skills_or_restart_gateways()
-> Result<(), Box<dyn Error + Send + Sync>> {
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
    // First apply creates the skill and starts the gateway in the stub.
    let (status, value) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert_eq!(status, StatusCode::OK, "first apply failed: {value}");
    assert_eq!(value["reconciliation"]["ok"], json!(true), "{value}");

    // Second, byte-identical apply must be a pure no-op against agent_host.
    server.state.clear_requests()?;
    let (status, value) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert_eq!(status, StatusCode::OK, "second apply failed: {value}");

    let recorded = server.state.recorded()?;
    let mutating: Vec<_> = recorded
        .iter()
        .filter(|(method, _)| method != Method::GET)
        .collect();
    assert!(
        mutating.is_empty(),
        "no-op apply performed mutating agent_host calls: {mutating:?}"
    );
    Ok(())
}

// ----- Item F: rollback keeps the document in sync -----

#[tokio::test]
async fn rollback_updates_config_document() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // Author a user skill (document + upstream).
    let (status, _value) = send(
        &app,
        json_post(
            "/skills",
            &json!({ "skill_id": "my-skill", "files": { "SKILL.md": "# original" } }),
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // The upstream rollback returns different file contents.
    server.state.seed_rollback(
        "my-skill",
        BTreeMap::from([("SKILL.md".to_owned(), "# rolled back".to_owned())]),
    );

    let (status, _value) = send(
        &app,
        json_post("/skills/my-skill/versions/1/rollback", &json!({}))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // The authoritative document must reflect the rolled-back files.
    let (status, body) = send_bytes(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert!(
        text.contains("# rolled back"),
        "document was not updated with rolled-back files: {text}"
    );
    Ok(())
}

// ----- Item G: referential integrity -----

#[tokio::test]
async fn create_agent_rejects_unknown_skill_reference() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let server = TestServer::start().await?;
    let app = server.app()?;

    let (status, value) = send(
        &app,
        json_post(
            "/agents",
            &json!({ "agent_id": "helper", "name": "Helper", "skills": ["ghost"] }),
        )?,
    )
    .await?;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unknown skill reference was accepted: {value}"
    );
    Ok(())
}

#[tokio::test]
async fn delete_referenced_skill_is_rejected() -> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    let app = server.app()?;

    // Author a user skill, then an agent that references it.
    let (status, _value) = send(
        &app,
        json_post(
            "/skills",
            &json!({ "skill_id": "shared", "files": { "SKILL.md": "# shared" } }),
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _value) = send(
        &app,
        json_post(
            "/agents",
            &json!({ "agent_id": "helper", "name": "Helper", "skills": ["shared"] }),
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // Deleting a referenced skill must be rejected.
    let (status, _value) =
        send(&app, Request::delete("/skills/shared").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::CONFLICT);

    // The skill and its upstream materialization must survive the rejection.
    assert!(
        server
            .state
            .skills
            .lock()
            .is_ok_and(|s| s.contains_key("shared"))
    );
    Ok(())
}

#[tokio::test]
async fn create_skill_colliding_with_builtin_is_rejected()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    // Seed an installation-owned builtin.
    server
        .state
        .seed_skill("builtin-skill", "builtin", BTreeMap::new());
    let app = server.app()?;

    let (status, _value) = send(
        &app,
        json_post(
            "/skills",
            &json!({ "skill_id": "builtin-skill", "files": { "SKILL.md": "# collide" } }),
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    Ok(())
}

#[tokio::test]
async fn apply_rejects_user_skill_colliding_with_builtin()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    server
        .state
        .seed_skill("builtin-skill", "builtin", BTreeMap::new());
    let app = server.app()?;

    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: builtin-skill
      files:
        SKILL.md: '# collide'
";
    let (status, value) = send(&app, yaml_post("/config/validate", source)?).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{value}");
    let issues = value["issues"].as_array().cloned().unwrap_or_default();
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "builtin_skill_collision"),
        "expected builtin_skill_collision: {value}"
    );
    Ok(())
}

// ----- Item H: ConfigDocument authority for skills -----

#[tokio::test]
async fn skills_are_read_from_document_not_stale_upstream()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let server = TestServer::start().await?;
    server
        .state
        .seed_skill("builtin-skill", "builtin", BTreeMap::new());
    let app = server.app()?;

    // Author a user skill through the document.
    let (status, _value) = send(
        &app,
        json_post(
            "/skills",
            &json!({ "skill_id": "doc-skill", "files": { "SKILL.md": "# doc" } }),
        )?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // Make the upstream drift: change doc-skill's files and add a phantom user
    // skill that the document knows nothing about.
    server.state.seed_skill(
        "doc-skill",
        "user",
        BTreeMap::from([("SKILL.md".to_owned(), "# STALE".to_owned())]),
    );
    server.state.seed_skill(
        "phantom",
        "user",
        BTreeMap::from([("SKILL.md".to_owned(), "# phantom".to_owned())]),
    );

    // GET of the authored skill must return the document files, not upstream.
    let (status, value) =
        send(&app, Request::get("/skills/doc-skill").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["files"]["SKILL.md"], json!("# doc"), "{value}");
    assert_eq!(value["source"], json!("user"));

    // The list must contain the document user skill and the builtin, but never
    // the stale upstream-only user skill.
    let (status, value) = send(&app, Request::get("/skills").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<String> = value
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|entry| entry["skill_id"].as_str().map(ToOwned::to_owned))
        .collect();
    assert!(ids.contains(&"doc-skill".to_owned()), "{ids:?}");
    assert!(ids.contains(&"builtin-skill".to_owned()), "{ids:?}");
    assert!(
        !ids.contains(&"phantom".to_owned()),
        "stale upstream user skill leaked into the listing: {ids:?}"
    );
    Ok(())
}

// ----- Item I: Git Agent secret-reference preservation -----

#[tokio::test]
async fn git_agent_secret_ref_survives_unrelated_update() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let server = TestServer::start().await?;
    let app = server.app()?;

    // Declare and set the secret that the Git Agent remote URL references.
    let (status, _value) = send(
        &app,
        json_post("/secrets", &json!({ "name": "GIT_REMOTE" }))?,
    )
    .await?;
    assert!(status.is_success(), "declaring secret failed: {status}");
    let (status, _value) = send(
        &app,
        json_put(
            "/secrets/GIT_REMOTE/value",
            &json!({ "value": "https://git.example/repo.git" }),
        )?,
    )
    .await?;
    assert!(status.is_success(), "setting secret value failed");

    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  secrets:
    - name: GIT_REMOTE
  agents:
    - id: reviewer
      name: Reviewer
      harness: acp
      systemPrompt: review carefully
  gitAgent:
    enabled: true
    defaultBranch: main
    remoteUrl:
      secretRef: GIT_REMOTE
    patchUrl: https://git.example/patch
    reviewAgent: reviewer
";
    let (status, value) = send(&app, yaml_post("/config/apply", source)?).await?;
    assert_eq!(status, StatusCode::OK, "git agent apply failed: {value}");

    // An unrelated PATCH that only changes the default branch must preserve the
    // secretRef-backed remote URL.
    let (status, value) = send(
        &app,
        json_put("/git-agent/config", &json!({ "default_branch": "trunk" }))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "git agent patch failed: {value}");

    let (status, body) = send_bytes(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert!(
        text.contains("secretRef: GIT_REMOTE"),
        "remoteUrl secretRef was lost by an unrelated update: {text}"
    );
    assert!(
        text.contains("defaultBranch: trunk"),
        "unrelated update was not applied: {text}"
    );
    Ok(())
}
