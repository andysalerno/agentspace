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

const SAMPLE_SOURCE: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  secrets:
    - name: OPENAI_KEY
      description: OpenAI key
  connections:
    - id: openai
      name: OpenAI
      url: https://api.openai.com
      apiFlavor: chat_completions
      apiKey:
        secretRef: OPENAI_KEY
  agents:
    - id: helper
      name: Helper
      harness: acp
      connection: openai
      systemPrompt: be helpful
  gateways:
    - id: echo-gw
      name: Echo
      type: echo
      agent: helper
      enabled: true
";

fn router() -> Result<Router, Box<dyn Error + Send + Sync>> {
    // These tests exercise the ConfigDocument/apply/export behavior. Apply
    // requires a reachable agent_host to determine the upstream user-skill state
    // (a missing/unreachable host is a hard failure), so stand up a minimal stub
    // that reports empty skill/gateway state and accepts staging calls.
    let base_url = spawn_stub_agent_host()?;
    let config = AppConfig::new("127.0.0.1", 0, &base_url, BTreeMap::new());
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
        .route("/skills", get(stub_list_skills).post(stub_ok_json))
        .route(
            "/skills/{skill_id}",
            get(stub_get_skill)
                .put(stub_ok_json)
                .delete(stub_no_content),
        )
        .route(
            "/gateways",
            get(stub_list_gateways).post(stub_create_gateway),
        )
        .route("/gateways/{gateway_id}", delete(stub_no_content));
    tokio::spawn(async move {
        let listener = TcpListener::from_std(listener)?;
        axum::serve(listener, app).await
    });
    Ok(format!("http://{address}"))
}

async fn stub_list_skills() -> Json<Value> {
    Json(json!([]))
}

async fn stub_get_skill(Path(skill_id): Path<String>) -> Json<Value> {
    Json(json!({ "skill_id": skill_id, "source": "user", "files": {} }))
}

async fn stub_list_gateways() -> Json<Value> {
    Json(json!([]))
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

fn apply_request(source: &str) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::post("/config/apply")
        .header(header::CONTENT_TYPE, "application/yaml")
        .body(Body::from(source.to_owned()))?)
}

#[tokio::test]
async fn apply_then_export_source_is_byte_identical() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;

    let (status, _headers, _body) = send(&app, apply_request(SAMPLE_SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    let (status, headers, body) =
        send(&app, Request::get("/config/export").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, SAMPLE_SOURCE.as_bytes());
    assert_eq!(
        headers
            .get(header::CONTENT_DISPOSITION.as_str())
            .map(String::as_str),
        Some("attachment; filename=\"agentspace-config.yaml\"")
    );

    // Explicit mode=source is equivalent to the default and byte-identical.
    let (status, _headers, explicit) = send(
        &app,
        Request::get("/config/export?mode=source").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(explicit, SAMPLE_SOURCE.as_bytes());
    Ok(())
}

#[tokio::test]
async fn canonical_export_is_stable_across_reapply() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    send(&app, apply_request(SAMPLE_SOURCE)?).await?;

    let (status, _headers, canonical_a) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let canonical_text = String::from_utf8(canonical_a.clone())?;
    let (status, _headers, _body) = send(&app, apply_request(&canonical_text)?).await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _headers, canonical_b) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(canonical_a, canonical_b);

    // After re-applying the canonical form, the exact source is that canonical text.
    let (status, _headers, source) =
        send(&app, Request::get("/config/export").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(source, canonical_b);
    Ok(())
}

#[tokio::test]
async fn per_resource_export_returns_standalone_manifest()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    send(&app, apply_request(SAMPLE_SOURCE)?).await?;

    let (status, headers, body) = send(
        &app,
        Request::get("/config/export/agent/helper").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert!(text.contains("kind: Agent"), "unexpected manifest: {text}");
    assert!(text.contains("name: helper"), "unexpected manifest: {text}");
    assert_eq!(
        headers
            .get(header::CONTENT_DISPOSITION.as_str())
            .map(String::as_str),
        Some("attachment; filename=\"agent-helper.yaml\"")
    );

    let (status, _headers, _body) = send(
        &app,
        Request::get("/config/export/agent/missing").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn per_resource_export_accepts_kebab_case_kinds() -> Result<(), Box<dyn Error + Send + Sync>>
{
    let app = router()?;
    let source = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  kernelConfigs:
    - harness: acp
      envText: A=B
  agents:
    - id: helper
      name: Helper
      harness: acp
      systemPrompt: be helpful
";
    let (status, _headers, _body) = send(&app, apply_request(source)?).await?;
    assert_eq!(status, StatusCode::OK);

    let (status, headers, body) = send(
        &app,
        Request::get("/config/export/kernel-config/acp").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert!(
        text.contains("kind: KernelConfig"),
        "unexpected manifest: {text}"
    );
    assert_eq!(
        headers
            .get(header::CONTENT_DISPOSITION.as_str())
            .map(String::as_str),
        Some("attachment; filename=\"kernel-config-acp.yaml\"")
    );

    Ok(())
}

#[tokio::test]
async fn apply_is_complete_replacement() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    send(&app, apply_request(SAMPLE_SOURCE)?).await?;

    let replacement = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  agents:
    - id: solo
      name: Solo
      harness: acp
      systemPrompt: only agent
";
    let (status, _headers, _body) = send(&app, apply_request(replacement)?).await?;
    assert_eq!(status, StatusCode::OK);

    let (status, _headers, body) = send(&app, Request::get("/agents").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    let agents: Value = serde_json::from_slice(&body)?;
    let ids: Vec<&str> = agents
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| agent["agent_id"].as_str())
        .collect();
    assert_eq!(ids, vec!["solo"]);

    // The previous connection and gateway are gone.
    let (status, _headers, _body) = send(
        &app,
        Request::get("/connections/openai").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn apply_rejects_invalid_graph_with_issues() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let invalid = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  gateways:
    - id: bad-gw
      name: Bad
      type: echo
      agent: nonexistent
      enabled: true
";
    let (status, _headers, body) = send(&app, apply_request(invalid)?).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let value: Value = serde_json::from_slice(&body)?;
    let issues = value["issues"].as_array().cloned().unwrap_or_default();
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "unresolved_agent_reference"),
        "expected unresolved_agent_reference, got: {value}"
    );
    Ok(())
}

#[tokio::test]
async fn validate_and_plan_report_unset_secrets() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;

    let (status, body) = {
        let (status, _headers, body) = send(
            &app,
            Request::post("/config/validate")
                .header(header::CONTENT_TYPE, "application/yaml")
                .body(Body::from(SAMPLE_SOURCE.to_owned()))?,
        )
        .await?;
        (status, body)
    };
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["valid"], json!(true));
    assert_eq!(value["unset_secrets"], json!(["OPENAI_KEY"]));

    let (status, _headers, body) = send(
        &app,
        Request::post("/config/plan")
            .header(header::CONTENT_TYPE, "application/yaml")
            .body(Body::from(SAMPLE_SOURCE.to_owned()))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert!(value["plan"].is_object() || value["plan"].is_array());
    Ok(())
}

#[tokio::test]
async fn secret_lifecycle_is_write_only() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;

    let (status, _headers, _body) = send(
        &app,
        Request::post("/secrets")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "name": "MY_SECRET", "description": "test" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _headers, body) =
        send(&app, Request::get("/secrets").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    let entry = &value
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(entry["name"], "MY_SECRET");
    assert_eq!(entry["is_set"], json!(false));
    assert!(entry.get("value").is_none());

    let (status, _headers, _body) = send(
        &app,
        Request::put("/secrets/MY_SECRET/value")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "value": "s3cr3t" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _headers, body) =
        send(&app, Request::get("/secrets").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    let text = String::from_utf8(body.clone())?;
    assert!(!text.contains("s3cr3t"), "value leaked in listing: {text}");
    let entry = &value
        .as_array()
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(entry["is_set"], json!(true));

    // Cannot delete a declaration while a value is set.
    let (status, _headers, _body) = send(
        &app,
        Request::delete("/secrets/MY_SECRET").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _headers, _body) = send(
        &app,
        Request::delete("/secrets/MY_SECRET/value").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _headers, _body) = send(
        &app,
        Request::delete("/secrets/MY_SECRET").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    Ok(())
}

#[tokio::test]
async fn set_value_requires_declaration() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let (status, _headers, _body) = send(
        &app,
        Request::put("/secrets/UNDECLARED/value")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "value": "x" }))?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn apply_omitting_set_declaration_returns_conflict()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    send(&app, apply_request(SAMPLE_SOURCE)?).await?;

    let (status, _headers, _body) = send(
        &app,
        Request::put("/secrets/OPENAI_KEY/value")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&json!({ "value": "live" }))?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A replacement that omits the set declaration must be rejected.
    let without_secret = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  agents:
    - id: solo
      name: Solo
      harness: acp
      systemPrompt: only agent
";
    let (status, _headers, _body) = send(&app, apply_request(without_secret)?).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    Ok(())
}

const SECRET_RICH_SOURCE: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  secrets:
    - name: OPENAI_KEY
      description: OpenAI key
    - name: GW_TOKEN
      description: Gateway token
  connections:
    - id: openai
      name: OpenAI
      url: https://api.openai.com
      apiFlavor: chat_completions
      apiKey:
        secretRef: OPENAI_KEY
  agents:
    - id: helper
      name: Helper
      harness: acp
      connection: openai
      systemPrompt: be helpful
      env:
        API_TOKEN:
          secretRef: OPENAI_KEY
        PLAIN: literal-value
  gateways:
    - id: echo-gw
      name: Echo
      type: echo
      agent: helper
      enabled: false
      secrets:
        GW_TOKEN:
          secretRef: GW_TOKEN
";

#[tokio::test]
async fn crud_patches_preserve_secret_refs_and_structured_env()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let (status, _headers, _body) = send(&app, apply_request(SECRET_RICH_SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    // Rename the connection via PATCH; the legacy request cannot carry the
    // secretRef, so the adapter must preserve it.
    let (status, _headers, _body) = send(
        &app,
        Request::patch("/connections/openai")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "name": "OpenAI Renamed" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // Rename the gateway via PATCH (an unrelated field).
    let (status, _headers, _body) = send(
        &app,
        Request::patch("/gateways/echo-gw")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "name": "Echo Renamed" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // Rename the agent via PATCH.
    let (status, _headers, _body) = send(
        &app,
        Request::patch("/agents/helper")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "name": "Helper Renamed" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // Canonical export must still carry every secret reference and the renames.
    let (status, _headers, body) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    let ref_count = text.matches("secretRef: OPENAI_KEY").count();
    assert!(
        ref_count >= 2,
        "expected connection apiKey and agent env secretRef preserved, got: {text}"
    );
    assert!(
        text.contains("secretRef: GW_TOKEN"),
        "expected gateway secretRef preserved, got: {text}"
    );
    assert!(
        text.contains("PLAIN: literal-value"),
        "expected literal env preserved, got: {text}"
    );
    assert!(text.contains("name: OpenAI Renamed"), "rename lost: {text}");
    assert!(text.contains("name: Echo Renamed"), "rename lost: {text}");
    assert!(text.contains("name: Helper Renamed"), "rename lost: {text}");
    Ok(())
}

#[tokio::test]
async fn lazy_resolution_reports_missing_secret() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    send(&app, apply_request(SAMPLE_SOURCE)?).await?;

    // OPENAI_KEY is declared but unset; model discovery must fail with 409.
    let (status, _headers, body) = send(
        &app,
        Request::get("/connections/openai/models").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["error"], "secret_values_unset");
    let missing = value["missing_secrets"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        missing.iter().any(|item| item["name"] == "OPENAI_KEY"),
        "expected OPENAI_KEY missing, got: {value}"
    );
    Ok(())
}

/// The web UI (and any other client) selects a connection API key by declared
/// secret name. Literal keys stay YAML-only, so the request forms are mutually
/// exclusive and an unknown name is rejected before it can reach the document.
#[tokio::test]
async fn connection_api_key_secret_is_selectable_by_name()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let (status, _headers, _body) = send(&app, apply_request(SECRET_RICH_SOURCE)?).await?;
    assert_eq!(status, StatusCode::OK);

    // The configured reference is reported by name, never by value.
    let (status, _headers, body) = send(
        &app,
        Request::get("/connections/openai").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["api_key_secret"], json!("OPENAI_KEY"));
    assert_eq!(value["has_api_key"], json!(true));
    assert!(value.get("api_key").is_none());

    // Point the connection at a different declared secret.
    let (status, _headers, body) = send(
        &app,
        Request::patch("/connections/openai")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "api_key_secret": "GW_TOKEN" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["api_key_secret"], json!("GW_TOKEN"));

    let (status, _headers, body) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert_eq!(
        text.matches("secretRef: GW_TOKEN").count(),
        2,
        "expected connection apiKey and gateway secret to reference GW_TOKEN, got: {text}"
    );

    // A name that violates the secret-name grammar is rejected outright; it can
    // never reach the record, where the field is typed as a validated name.
    let (status, _headers, _body) = send(
        &app,
        Request::patch("/connections/openai")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "api_key_secret": "lower_case" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // An undeclared name is rejected rather than written to the document.
    let (status, _headers, _body) = send(
        &app,
        Request::patch("/connections/openai")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "api_key_secret": "NOT_DECLARED" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // A literal and a reference in one request is ambiguous. Presence decides,
    // not emptiness, so no combination of values slips through.
    for payload in [
        json!({ "api_key": "sk-literal", "api_key_secret": "OPENAI_KEY" }),
        json!({ "api_key": "", "api_key_secret": "OPENAI_KEY" }),
        json!({ "api_key": "sk-literal", "api_key_secret": "" }),
    ] {
        let (status, _headers, _body) = send(
            &app,
            Request::patch("/connections/openai")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload)?))?,
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "expected {payload} to be rejected"
        );
    }

    // Omitting both fields leaves the configured reference untouched.
    let (status, _headers, body) = send(
        &app,
        Request::patch("/connections/openai")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "name": "Renamed" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["api_key_secret"], json!("GW_TOKEN"));

    // The reference is immediately visible to declaration removal, which is the
    // observable half of serializing the write against secret operations: a
    // referenced declaration cannot be removed out from under the connection.
    let (status, _headers, _body) = send(
        &app,
        Request::delete("/secrets/GW_TOKEN").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);

    // An empty selection clears the reference entirely.
    let (status, _headers, body) = send(
        &app,
        Request::patch("/connections/openai")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "api_key_secret": "" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["has_api_key"], json!(false));
    assert_eq!(value["api_key_secret"], Value::Null);

    let (status, _headers, body) = send(
        &app,
        Request::get("/config/export?mode=canonical").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body)?;
    assert!(
        !text.contains("apiKey:"),
        "expected the connection apiKey to be removed, got: {text}"
    );
    Ok(())
}
