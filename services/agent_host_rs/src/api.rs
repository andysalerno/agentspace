use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{AppState, ENV_PREFIX};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/info", get(info))
        .merge(crate::sessions::router())
        .merge(crate::skills::router())
        .merge(crate::gateways::router())
}

async fn healthz() -> Json<HealthResponse> {
    tracing::debug!(
        route = "/healthz",
        action = "healthz",
        "api handler completed"
    );
    Json(HealthResponse { status: "ok" })
}

async fn info(State(state): State<AppState>) -> Json<Value> {
    tracing::info!(route = "/info", action = "info", "api handler completed");
    Json(json!({
        "service": "agent_host",
        "title": "Agent Host",
        "version": env!("CARGO_PKG_VERSION"),
        "env_prefix": ENV_PREFIX,
        "env": state.config.agent_host_env,
        "instance_id": state.instance_id,
        "started_at": state.started_at,
        "components": {
            "sessions": state.sessions.summary(),
            "docker_runtime": state.docker_runtime.summary(),
            "skills": state.skills.summary(),
            "gateways": state.gateways.summary(),
        },
    }))
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::{AppConfig, AppState, build_router};

    #[tokio::test]
    async fn healthz_returns_ok() {
        let app = build_router(AppState::new(AppConfig::new(
            "127.0.0.1",
            0,
            BTreeMap::new(),
        )));
        let request = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = app
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));

        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to parse response body: {error}"));
        assert_eq!(payload, serde_json::json!({ "status": "ok" }));
    }

    #[tokio::test]
    async fn info_reports_agent_host_metadata() {
        let mut env = BTreeMap::new();
        env.insert("AGENT_HOST_EXAMPLE".to_owned(), "enabled".to_owned());
        let app = build_router(AppState::new(AppConfig::new("127.0.0.1", 0, env)));
        let request = Request::builder()
            .uri("/info")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = app
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));

        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
            .to_bytes();
        let payload: Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to parse response body: {error}"));
        assert_eq!(payload["service"], "agent_host");
        assert_eq!(payload["env_prefix"], "AGENT_HOST_");
        assert_eq!(payload["env"]["AGENT_HOST_EXAMPLE"], "enabled");
    }
}
