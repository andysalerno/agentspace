#![allow(clippy::too_many_lines)]

use std::{collections::BTreeMap, error::Error};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use client_service_rs::{AppConfig, AppState, build_router};
use serde_json::Value;
use tower::ServiceExt;

fn router() -> Result<Router, Box<dyn Error + Send + Sync>> {
    let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", BTreeMap::new());
    Ok(build_router(AppState::new(config)?))
}

async fn validate(
    app: &Router,
    source: &str,
) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
    let request = Request::post("/config/validate")
        .header(header::CONTENT_TYPE, "application/yaml")
        .body(Body::from(source.to_owned()))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await?;
    let parsed: Value = serde_json::from_slice(&body)?;
    Ok((status, parsed))
}

fn issue_codes(parsed: &Value) -> Vec<String> {
    parsed["issues"]
        .as_array()
        .map(|issues| {
            issues
                .iter()
                .filter_map(|issue| issue["code"].as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn valid_skill_passes_validation() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let source = r##"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: helper
      files:
        SKILL.md: "# Helper"
        agentspace.json: '{"schema_version":1,"resources":{"volumes":[{"id":"v1","scope":"installation","mount_path":"/data/helper","mode":"ro"}]}}'
"##;
    let (status, parsed) = validate(&app, source).await?;
    assert_eq!(status, StatusCode::OK, "body: {parsed}");
    assert_eq!(parsed["valid"], true);
    Ok(())
}

#[tokio::test]
async fn skill_missing_markdown_is_rejected() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let source = r#"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: helper
      files:
        NOTES.md: "no entry point"
"#;
    let (status, parsed) = validate(&app, source).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(issue_codes(&parsed).contains(&"missing_skill_markdown".to_owned()));
    Ok(())
}

#[tokio::test]
async fn skill_path_traversal_is_rejected() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let source = r##"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: helper
      files:
        SKILL.md: "# Helper"
        "../escape.txt": "traversal"
"##;
    let (status, parsed) = validate(&app, source).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(issue_codes(&parsed).contains(&"invalid_skill_file_path".to_owned()));
    Ok(())
}

#[tokio::test]
async fn skill_reserved_mount_path_is_rejected() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let source = r##"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: helper
      files:
        SKILL.md: "# Helper"
        agentspace.json: '{"schema_version":1,"resources":{"volumes":[{"id":"v1","scope":"installation","mount_path":"/workspace","mode":"ro"}]}}'
"##;
    let (status, parsed) = validate(&app, source).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(issue_codes(&parsed).contains(&"invalid_skill_mount_path".to_owned()));
    Ok(())
}

#[tokio::test]
async fn cross_skill_mount_collision_is_rejected() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let source = r##"apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  skills:
    - id: alpha
      files:
        SKILL.md: "# Alpha"
        agentspace.json: '{"schema_version":1,"resources":{"volumes":[{"id":"a","scope":"installation","mount_path":"/data/shared","mode":"ro"}]}}'
    - id: beta
      files:
        SKILL.md: "# Beta"
        agentspace.json: '{"schema_version":1,"resources":{"volumes":[{"id":"b","scope":"installation","mount_path":"/data/shared","mode":"ro"}]}}'
"##;
    let (status, parsed) = validate(&app, source).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(issue_codes(&parsed).contains(&"skill_mount_path_collision".to_owned()));
    Ok(())
}
