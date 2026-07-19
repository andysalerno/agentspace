#![allow(clippy::too_many_lines)]

use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{DefaultBodyLimit, OriginalUri, State},
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use client_service_rs::{AppConfig, AppState, build_router, memory::MEMORY_RUN_CONTENT_TYPE};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct StubState {
    page_queries: Arc<Mutex<Vec<Option<String>>>>,
}

struct StubMemory {
    base_url: String,
    state: StubState,
    handle: JoinHandle<Result<(), std::io::Error>>,
}

impl StubMemory {
    async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let state = StubState::default();
        let app = Router::new()
            .route(
                "/healthz",
                get(|| async { Json(json!({ "status": "ok" })) }),
            )
            .route("/v1/pages", get(stub_pages))
            .route("/v1/pages/content", put(stub_conflict))
            .route("/v1/tags", get(stub_slow_tags))
            .route("/v1/check", get(stub_malformed))
            .route("/v1/run", post(stub_run))
            .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let handle = tokio::spawn(axum::serve(listener, app).into_future());
        Ok(Self {
            base_url: format!("http://{address}"),
            state,
            handle,
        })
    }

    fn app(&self, timeout: Duration) -> Result<Router, Box<dyn Error + Send + Sync>> {
        let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", BTreeMap::new())
            .with_memory_base_url(&self.base_url)
            .with_memory_timeout(timeout);
        Ok(build_router(AppState::new(config)?))
    }

    fn page_queries(&self) -> Result<Vec<Option<String>>, Box<dyn Error + Send + Sync>> {
        self.state
            .page_queries
            .lock()
            .map(|queries| queries.clone())
            .map_err(|_error| "page query lock poisoned".into())
    }
}

impl Drop for StubMemory {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn stub_pages(
    State(state): State<StubState>,
    OriginalUri(uri): OriginalUri,
) -> Result<Json<Value>, StatusCode> {
    state
        .page_queries
        .lock()
        .map_err(|_error| StatusCode::INTERNAL_SERVER_ERROR)?
        .push(uri.query().map(ToOwned::to_owned));
    Ok(Json(json!([{
        "path": "projects/agentspace",
        "title": "AgentSpace",
        "tags": ["project"],
        "updated_at": "2026-07-18T00:00:00Z"
    }])))
}

async fn stub_conflict(_body: Bytes) -> impl IntoResponse {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": {
                "kind": "conflict",
                "message": "stale revision"
            }
        })),
    )
}

async fn stub_slow_tags() -> Json<Value> {
    tokio::time::sleep(Duration::from_millis(100)).await;
    Json(json!([]))
}

async fn stub_malformed() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], "not-json").into_response()
}

async fn stub_run() -> Response {
    tokio::time::sleep(Duration::from_millis(30)).await;
    (
        [(header::CONTENT_TYPE, MEMORY_RUN_CONTENT_TYPE)],
        vec![0_u8, 255, 1, 128, b'x'],
    )
        .into_response()
}

async fn request(
    app: Router,
    request: Request<Body>,
) -> Result<(StatusCode, axum::http::HeaderMap, Vec<u8>), Box<dyn Error + Send + Sync>> {
    let response = app.oneshot(request).await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), 20 * 1024 * 1024)
        .await?
        .to_vec();
    Ok((status, headers, body))
}

#[tokio::test]
async fn proxies_json_queries_and_upstream_conflicts() -> Result<(), Box<dyn Error + Send + Sync>> {
    let stub = StubMemory::start().await?;
    let app = stub.app(Duration::from_secs(1))?;
    let (status, _headers, body) = request(
        app.clone(),
        Request::get("/memory/v1/pages?under=projects&limit=10").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value[0]["path"], "projects/agentspace");
    assert_eq!(
        stub.page_queries()?,
        vec![Some("under=projects&limit=10".to_owned())]
    );

    let (status, _headers, body) = request(
        app,
        Request::put("/memory/v1/pages/content?path=projects/agentspace")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(
                &json!({ "expected_revision": "stale" }),
            )?))?,
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let value: Value = serde_json::from_slice(&body)?;
    assert_eq!(value["error"]["kind"], "conflict");
    Ok(())
}

#[tokio::test]
async fn accepts_requests_up_to_the_memory_service_limit()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let stub = StubMemory::start().await?;
    let app = stub.app(Duration::from_secs(5))?;
    let body = serde_json::to_vec(&json!({
        "body": "x".repeat(3 * 1024 * 1024),
        "expected_revision": "stale"
    }))?;

    let (status, _headers, _body) = request(
        app,
        Request::put("/memory/v1/pages/content?path=projects/large")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))?,
    )
    .await?;

    assert_eq!(status, StatusCode::CONFLICT);
    Ok(())
}

#[tokio::test]
async fn streams_run_bytes_without_json_buffering() -> Result<(), Box<dyn Error + Send + Sync>> {
    let stub = StubMemory::start().await?;
    let (status, headers, body) = request(
        stub.app(Duration::from_millis(10))?,
        Request::post("/memory/v1/run")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"argv":["pwd"]}"#))?,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some(MEMORY_RUN_CONTENT_TYPE)
    );
    assert_eq!(body, vec![0_u8, 255, 1, 128, b'x']);
    Ok(())
}

#[tokio::test]
async fn maps_malformed_timeout_and_unavailable_upstreams()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let stub = StubMemory::start().await?;
    let app = stub.app(Duration::from_millis(10))?;

    let (status, _headers, body) = request(
        app.clone(),
        Request::get("/memory/v1/check").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let value: Value = serde_json::from_slice(&body)?;
    assert!(
        value["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("malformed"))
    );

    let (status, _headers, _body) =
        request(app, Request::get("/memory/v1/tags").body(Body::empty())?).await?;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);

    let config = AppConfig::new("127.0.0.1", 0, "http://127.0.0.1:9", BTreeMap::new())
        .with_memory_base_url("http://127.0.0.1:9")
        .with_memory_timeout(Duration::from_millis(100));
    let (status, _headers, _body) = request(
        build_router(AppState::new(config)?),
        Request::get("/memory/healthz").body(Body::empty())?,
    )
    .await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}
