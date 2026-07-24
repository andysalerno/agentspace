use std::{collections::BTreeMap, error::Error};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use client_service_rs::{AppConfig, AppState, build_router};
use tower::ServiceExt;

const WEBUI_ORIGIN: &str = "http://localhost:8003";
const EVIL_ORIGIN: &str = "https://evil.example.com";

fn router() -> Result<Router, Box<dyn Error + Send + Sync>> {
    let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", BTreeMap::new());
    Ok(build_router(AppState::new(config)?))
}

fn preflight(origin: &str) -> Result<Request<Body>, Box<dyn Error + Send + Sync>> {
    Ok(Request::builder()
        .method("OPTIONS")
        .uri("/config/export")
        .header(header::ORIGIN, origin)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
        .body(Body::empty())?)
}

#[tokio::test]
async fn preflight_allows_webui_origin() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let response = app.oneshot(preflight(WEBUI_ORIGIN)?).await?;
    let allow_origin = response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    assert_eq!(allow_origin.as_deref(), Some(WEBUI_ORIGIN));
    Ok(())
}

#[tokio::test]
async fn preflight_denies_evil_origin() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = router()?;
    let response = app.oneshot(preflight(EVIL_ORIGIN)?).await?;
    // An unlisted origin never receives an Access-Control-Allow-Origin header,
    // so the browser blocks the cross-origin request.
    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn request_without_origin_is_served() -> Result<(), Box<dyn Error + Send + Sync>> {
    // CLI / service-to-service callers send no Origin header and must be served
    // normally (CORS only governs browser cross-origin access).
    let app = router()?;
    let response = app
        .oneshot(Request::get("/config/export").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}
