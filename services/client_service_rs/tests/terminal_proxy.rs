#![allow(clippy::too_many_lines)]

use std::{
    collections::BTreeMap,
    error::Error,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{
        Path, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use client_service_rs::{AppConfig, AppState, agent_host::AgentHostClient, build_router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle, time::sleep};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Error as WebSocketError, Message as ClientMessage,
        client::IntoClientRequest,
        http::{HeaderValue, header::ORIGIN},
        protocol::{CloseFrame as ClientCloseFrame, frame::coding::CloseCode},
    },
};

const ALLOWED_ORIGIN: &str = "http://allowed.example";
const DENIED_ORIGIN: &str = "https://denied.example";
const UPSTREAM_BINARY: &[u8] = &[0, 0xff, b'A', 0x80];
const LIFECYCLE_FRAME: &str = r#"{"type":"exited","state":"exited","exit_status":7,"terminal":{"state":"exited","exit_status":7,"attach_kind":null,"attachment_count":0,"socket_path":"/run/agentspace-tmux.sock","pane_pid":4242}}"#;

#[derive(Clone, Debug, PartialEq)]
struct RecordedRequest {
    method: &'static str,
    path: String,
    body: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum WebSocketObservation {
    Binary(Vec<u8>),
    Text(String),
    Close(u16, String),
}

#[derive(Clone, Debug, Default)]
enum WebSocketMode {
    #[default]
    Normal,
    CloseOnConnect(u16, String),
    DropOnConnect,
}

#[derive(Clone, Default)]
struct StubState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    observations: Arc<Mutex<Vec<WebSocketObservation>>>,
    fail_create: Arc<AtomicBool>,
    terminal_unavailable: Arc<AtomicBool>,
    terminal_resumed: Arc<AtomicBool>,
    ensure_count: Arc<AtomicUsize>,
    websocket_mode: Arc<Mutex<WebSocketMode>>,
}

impl StubState {
    fn record(
        &self,
        method: &'static str,
        path: impl Into<String>,
        body: Option<Value>,
    ) -> Result<(), StatusCode> {
        self.requests
            .lock()
            .map_err(|_error| StatusCode::INTERNAL_SERVER_ERROR)?
            .push(RecordedRequest {
                method,
                path: path.into(),
                body,
            });
        Ok(())
    }

    fn requests(&self) -> Result<Vec<RecordedRequest>, Box<dyn Error + Send + Sync>> {
        Ok(self
            .requests
            .lock()
            .map_err(|_error| "request mutex poisoned")?
            .clone())
    }

    fn observations(&self) -> Result<Vec<WebSocketObservation>, Box<dyn Error + Send + Sync>> {
        Ok(self
            .observations
            .lock()
            .map_err(|_error| "observation mutex poisoned")?
            .clone())
    }

    fn set_websocket_mode(&self, mode: WebSocketMode) -> Result<(), Box<dyn Error + Send + Sync>> {
        *self
            .websocket_mode
            .lock()
            .map_err(|_error| "websocket mode mutex poisoned")? = mode;
        Ok(())
    }
}

struct TestServer {
    base_url: String,
    handle: JoinHandle<Result<(), std::io::Error>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct TestHarness {
    client: reqwest::Client,
    client_server: TestServer,
    upstream_server: TestServer,
    upstream: StubState,
}

impl TestHarness {
    async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let upstream = StubState::default();
        let upstream_server = spawn_server(stub_router(upstream.clone())).await?;
        let config = AppConfig::new(
            "127.0.0.1",
            0,
            &upstream_server.base_url,
            BTreeMap::from([(
                "CLIENT_SERVICE_SECRET_KEY".to_owned(),
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
            )]),
        )
        .with_cors_allowed_origins([ALLOWED_ORIGIN]);
        let agent_host = AgentHostClient::new(&upstream_server.base_url, Duration::from_secs(2))?;
        let app = build_router(AppState::with_agent_host(config, agent_host)?);
        let client_server = spawn_server(app).await?;
        Ok(Self {
            client: reqwest::Client::new(),
            client_server,
            upstream_server,
            upstream,
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        let request = self
            .client
            .request(method, format!("{}{}", self.client_server.base_url, path));
        let response = match body {
            Some(body) => request.json(&body).send().await?,
            None => request.send().await?,
        };
        let status = response.status();
        let bytes = response.bytes().await?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        };
        Ok((status, value))
    }

    async fn post(
        &self,
        path: &str,
        body: Value,
    ) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        self.request(reqwest::Method::POST, path, Some(body)).await
    }

    async fn get(&self, path: &str) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        self.request(reqwest::Method::GET, path, None).await
    }

    async fn patch(
        &self,
        path: &str,
        body: Value,
    ) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        self.request(reqwest::Method::PATCH, path, Some(body)).await
    }

    async fn put(
        &self,
        path: &str,
        body: Value,
    ) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        self.request(reqwest::Method::PUT, path, Some(body)).await
    }

    async fn delete(
        &self,
        path: &str,
    ) -> Result<(StatusCode, Value), Box<dyn Error + Send + Sync>> {
        self.request(reqwest::Method::DELETE, path, None).await
    }

    async fn create_cli_session(&self) -> Result<(String, Value), Box<dyn Error + Send + Sync>> {
        let (status, _connection) = self
            .post(
                "/connections",
                json!({
                    "connection_id": "provider",
                    "name": "Provider",
                    "url": "https://provider.example/v1",
                    "api_flavor": "responses",
                    "api_key": "initial-key",
                }),
            )
            .await?;
        assert_eq!(status, StatusCode::OK);
        let (status, _agent) = self
            .post(
                "/agents",
                json!({
                    "agent_id": "cli-agent",
                    "name": "CLI Agent",
                    "system_prompt": "Original prompt",
                    "env_vars": concat!(
                        "COPILOT_MODEL=original-model\n",
                        "COPILOT_REASONING_EFFORT=high\n",
                        "COPILOT_ADDITIONAL_PATHS=/workspace/extra"
                    ),
                    "cli": {
                        "harness": "copilot-cli",
                        "connection_id": "provider",
                    },
                }),
            )
            .await?;
        assert_eq!(status, StatusCode::OK);
        let (status, session) = self
            .post(
                "/sessions",
                json!({
                    "agent_id": "cli-agent",
                    "client_type": "webui",
                    "interaction_mode": "cli",
                }),
            )
            .await?;
        assert_eq!(status, StatusCode::OK, "{session}");
        let session_id = string_field(&session, "session_id")?;
        Ok((session_id, session))
    }

    fn websocket_url(&self, session_id: &str) -> String {
        self.client_server.base_url.replacen("http://", "ws://", 1)
            + &format!("/sessions/{session_id}/terminal/ws")
    }
}

async fn spawn_server(app: Router) -> Result<TestServer, Box<dyn Error + Send + Sync>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let handle = tokio::spawn(async move { axum::serve(listener, app).await });
    Ok(TestServer {
        base_url: format_base_url(address),
        handle,
    })
}

fn format_base_url(address: SocketAddr) -> String {
    format!("http://{address}")
}

fn stub_router(state: StubState) -> Router {
    Router::new()
        .route("/sessions", post(create_upstream_session))
        .route("/sessions/{session_id}", get(get_upstream_session))
        .route(
            "/sessions/{session_id}/messages/stream",
            post(stream_upstream_message),
        )
        .route(
            "/sessions/{session_id}/terminal",
            get(upstream_terminal_status),
        )
        .route(
            "/sessions/{session_id}/terminal/ensure",
            post(upstream_terminal_ensure),
        )
        .route(
            "/sessions/{session_id}/terminal/stop",
            post(upstream_terminal_stop),
        )
        .route(
            "/sessions/{session_id}/terminal/resume",
            post(upstream_terminal_resume),
        )
        .route(
            "/sessions/{session_id}/terminal/ws",
            get(upstream_terminal_websocket),
        )
        .with_state(state)
}

async fn create_upstream_session(
    State(state): State<StubState>,
    Json(body): Json<Value>,
) -> Response {
    if let Err(status) = state.record("POST", "/sessions", Some(body.clone())) {
        return status.into_response();
    }
    if state.fail_create.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(json!({
        "session_id": body["session_id"],
        "status": "idle",
        "vscode_url": "http://127.0.0.1:45678",
        "free_port_url": "http://127.0.0.1:45679",
    }))
    .into_response()
}

async fn get_upstream_session(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    state.record("GET", format!("/sessions/{session_id}"), None)?;
    Ok(Json(json!({ "session_id": session_id, "status": "idle" })))
}

async fn stream_upstream_message(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, StatusCode> {
    state.record(
        "POST",
        format!("/sessions/{session_id}/messages/stream"),
        Some(body),
    )?;
    Ok((
        StatusCode::OK,
        [("content-type", "application/x-ndjson")],
        Body::from("{\"type\":\"text_delta\",\"content\":\"chat-ok\"}\n"),
    )
        .into_response())
}

fn terminal_status(attach_kind: Option<&str>, state: &str) -> Value {
    json!({
        "state": state,
        "exit_status": if state == "exited" { Some(7) } else { None },
        "attach_kind": attach_kind,
        "session_name": "agentspace-test",
        "target_session": "agentspace-test:0",
        "socket_path": "/run/agentspace-tmux.sock",
        "attach_argv": ["tmux", "attach"],
        "pane_id": "%0",
        "pane_pid": 42,
        "attachment_count": 0,
        "clients": [],
    })
}

fn terminal_response(
    state: &StubState,
    attach_kind: Option<&str>,
    terminal_state: &str,
) -> Response {
    if state.terminal_unavailable.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(terminal_status(attach_kind, terminal_state)).into_response()
}

async fn upstream_terminal_status(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
) -> Response {
    if let Err(status) = state.record("GET", format!("/sessions/{session_id}/terminal"), None) {
        return status.into_response();
    }
    let attach_kind = state
        .terminal_resumed
        .load(Ordering::SeqCst)
        .then_some("resumed");
    terminal_response(&state, attach_kind, "running")
}

async fn upstream_terminal_ensure(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
) -> Response {
    if let Err(status) = state.record(
        "POST",
        format!("/sessions/{session_id}/terminal/ensure"),
        None,
    ) {
        return status.into_response();
    }
    let count = state.ensure_count.fetch_add(1, Ordering::SeqCst);
    terminal_response(
        &state,
        Some(if count == 0 { "started" } else { "attached" }),
        "running",
    )
}

async fn upstream_terminal_stop(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
) -> Response {
    if let Err(status) = state.record(
        "POST",
        format!("/sessions/{session_id}/terminal/stop"),
        None,
    ) {
        return status.into_response();
    }
    terminal_response(&state, None, "exited")
}

async fn upstream_terminal_resume(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
) -> Response {
    if let Err(status) = state.record(
        "POST",
        format!("/sessions/{session_id}/terminal/resume"),
        None,
    ) {
        return status.into_response();
    }
    state.terminal_resumed.store(true, Ordering::SeqCst);
    terminal_response(&state, Some("resumed"), "running")
}

async fn upstream_terminal_websocket(
    State(state): State<StubState>,
    Path(session_id): Path<String>,
    websocket: WebSocketUpgrade,
) -> Response {
    if state.terminal_unavailable.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if let Err(status) = state.record("GET", format!("/sessions/{session_id}/terminal/ws"), None) {
        return status.into_response();
    }
    websocket.on_upgrade(move |socket| serve_upstream_websocket(state, socket))
}

async fn serve_upstream_websocket(state: StubState, mut socket: WebSocket) {
    let mode = state
        .websocket_mode
        .lock()
        .map_or(WebSocketMode::DropOnConnect, |mode| mode.clone());
    match mode {
        WebSocketMode::CloseOnConnect(code, reason) => {
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                })))
                .await;
        }
        WebSocketMode::DropOnConnect => {}
        WebSocketMode::Normal => {
            let ready = json!({
                "type": "ready",
                "attachment_id": "attachment-one",
                "cols": 80,
                "rows": 24,
                "terminal": terminal_status(Some("attached"), "running"),
            })
            .to_string();
            if socket.send(Message::Text(ready.into())).await.is_err()
                || socket
                    .send(Message::Binary(UPSTREAM_BINARY.to_vec().into()))
                    .await
                    .is_err()
                || socket
                    .send(Message::Text(LIFECYCLE_FRAME.to_owned().into()))
                    .await
                    .is_err()
            {
                return;
            }
            while let Some(message) = socket.next().await {
                match message {
                    Ok(Message::Binary(bytes)) => {
                        if let Ok(mut observations) = state.observations.lock() {
                            observations.push(WebSocketObservation::Binary(bytes.to_vec()));
                        }
                        if socket.send(Message::Binary(bytes)).await.is_err() {
                            return;
                        }
                    }
                    Ok(Message::Text(text)) => {
                        if let Ok(mut observations) = state.observations.lock() {
                            observations.push(WebSocketObservation::Text(text.as_str().to_owned()));
                        }
                        if socket.send(Message::Text(text)).await.is_err() {
                            return;
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        if let Some(frame) = &frame
                            && let Ok(mut observations) = state.observations.lock()
                        {
                            observations.push(WebSocketObservation::Close(
                                frame.code,
                                frame.reason.as_str().to_owned(),
                            ));
                        }
                        let _ = socket.send(Message::Close(frame)).await;
                        return;
                    }
                    Ok(Message::Ping(bytes)) => {
                        let _ = socket.send(Message::Pong(bytes)).await;
                    }
                    Ok(Message::Pong(_)) => {}
                    Err(_error) => return,
                }
            }
        }
    }
}

fn string_field(value: &Value, field: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("missing string field {field}").into())
}

async fn connect_terminal(
    url: &str,
    origin: Option<&str>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WebSocketError,
> {
    let mut request = url.into_client_request()?;
    if let Some(origin) = origin {
        let origin = HeaderValue::from_str(origin)
            .unwrap_or_else(|error| panic!("invalid test Origin header: {error}"));
        request.headers_mut().insert(ORIGIN, origin);
    }
    connect_async(request)
        .await
        .map(|(socket, _response)| socket)
}

async fn websocket_status(url: &str, origin: Option<&str>) -> StatusCode {
    match connect_terminal(url, origin).await {
        Err(WebSocketError::Http(response)) => response.status(),
        Ok(mut socket) => {
            let _ = socket.close(None).await;
            panic!("WebSocket unexpectedly upgraded")
        }
        Err(error) => panic!("unexpected WebSocket error: {error}"),
    }
}

#[tokio::test]
async fn cli_creation_controls_and_repeated_ensure_use_stable_snapshot()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let harness = TestHarness::start().await?;
    let (session_id, session) = harness.create_cli_session().await?;
    assert_eq!(session["runtime_status"], "live");
    assert_eq!(session["status"], "running");
    assert!(session.get("agent_host_session_id").is_none());
    assert_eq!(session["vscode_url"], "http://127.0.0.1:45678");
    assert_eq!(session["free_port_url"], "http://127.0.0.1:45679");
    let harness_session_id = string_field(&session, "harness_session_id")?;

    let (status, terminal) = harness
        .get(&format!("/sessions/{session_id}/terminal"))
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(terminal["state"], "running");
    for internal in [
        "session_name",
        "target_session",
        "socket_path",
        "attach_argv",
        "pane_id",
        "pane_pid",
        "clients",
    ] {
        assert!(
            terminal.get(internal).is_none(),
            "leaked terminal field {internal}"
        );
    }

    let (status, _updated) = harness
        .patch(
            "/connections/provider",
            json!({
                "url": "https://changed.example/v1",
                "api_key": "rotated-key",
            }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _updated) = harness
        .patch(
            "/agents/cli-agent",
            json!({
                "env_vars": "COPILOT_MODEL=changed-model",
                "system_prompt": "Changed prompt",
            }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);

    for _ in 0..2 {
        let (status, ensured) = harness
            .post(
                &format!("/sessions/{session_id}/terminal/ensure"),
                json!({}),
            )
            .await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ensured["state"], "running");
    }
    let (status, stopped) = harness
        .post(&format!("/sessions/{session_id}/terminal/stop"), json!({}))
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped["state"], "exited");
    let (status, resumed) = harness
        .post(
            &format!("/sessions/{session_id}/terminal/resume"),
            json!({}),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resumed["attach_kind"], "resumed");
    let (status, updated_session) = harness.get(&format!("/sessions/{session_id}")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated_session["runtime_generation"], 1);

    let session_creates = harness
        .upstream
        .requests()?
        .into_iter()
        .filter(|request| request.method == "POST" && request.path == "/sessions")
        .collect::<Vec<_>>();
    assert_eq!(session_creates.len(), 3);
    for request in &session_creates {
        let body = request.body.as_ref().ok_or("missing create body")?;
        assert_eq!(body["session_id"], session_id);
        assert_eq!(body["interaction_mode"], "cli");
        assert_eq!(body["harness"], "copilot-cli");
        assert_eq!(body["env"]["KERNEL_SESSION_ID"], harness_session_id);
        assert_eq!(body["env"]["CONNECTION_URL"], "https://provider.example/v1");
        assert_eq!(body["env"]["COPILOT_MODEL"], "original-model");
        assert_eq!(body["env"]["KERNEL_SYSTEM_PROMPT"], "Original prompt");
        assert_eq!(
            body["additional_paths"],
            json!(["/workspace", "/workspace/extra"])
        );
    }
    assert_eq!(
        session_creates[0].body.as_ref().ok_or("missing body")?["env"]["CONNECTION_API_KEY"],
        "initial-key"
    );
    assert_eq!(
        session_creates[1].body.as_ref().ok_or("missing body")?["env"]["CONNECTION_API_KEY"],
        "rotated-key"
    );
    assert_eq!(
        session_creates[2].body.as_ref().ok_or("missing body")?["env"]["CONNECTION_API_KEY"],
        "rotated-key"
    );
    assert!(harness.upstream.ensure_count.load(Ordering::SeqCst) >= 3);
    assert!(harness.upstream_server.base_url.starts_with("http://"));
    Ok(())
}

#[tokio::test]
async fn failed_cli_creation_is_retryable_without_false_success()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let harness = TestHarness::start().await?;
    harness.upstream.fail_create.store(true, Ordering::SeqCst);
    let (status, _connection) = harness
        .post(
            "/connections",
            json!({
                "connection_id": "provider",
                "name": "Provider",
                "url": "https://provider.example/v1",
                "api_flavor": "responses",
            }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _agent) = harness
        .post(
            "/agents",
            json!({
                "agent_id": "cli-agent",
                "name": "CLI Agent",
                "cli": { "harness": "copilot-cli", "connection_id": "provider" },
            }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, error) = harness
        .post(
            "/sessions",
            json!({ "agent_id": "cli-agent", "interaction_mode": "cli" }),
        )
        .await?;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{error}");

    let (status, sessions) = harness.get("/sessions").await?;
    assert_eq!(status, StatusCode::OK);
    let session_id = string_field(&sessions[0], "session_id")?;
    assert_eq!(sessions[0]["runtime_status"], "error");
    assert_eq!(sessions[0]["status"], "error");
    assert!(sessions[0].get("agent_host_session_id").is_none());

    harness.upstream.fail_create.store(false, Ordering::SeqCst);
    let (status, terminal) = harness
        .post(
            &format!("/sessions/{session_id}/terminal/ensure"),
            json!({}),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{terminal}");
    assert_eq!(terminal["state"], "running");
    let (status, recovered) = harness.get(&format!("/sessions/{session_id}")).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered["runtime_status"], "live");
    assert_eq!(recovered["status"], "running");
    Ok(())
}

#[tokio::test]
async fn terminal_recovery_reports_missing_secret_and_recovers_after_restore()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let harness = TestHarness::start().await?;
    let (status, _secret) = harness
        .post("/secrets", json!({ "name": "PROVIDER_KEY" }))
        .await?;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _empty) = harness
        .put(
            "/secrets/PROVIDER_KEY/value",
            json!({ "value": "initial-key" }),
        )
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _connection) = harness
        .post(
            "/connections",
            json!({
                "connection_id": "secret-provider",
                "name": "Secret Provider",
                "url": "https://provider.example/v1",
                "api_flavor": "responses",
                "api_key_secret": "PROVIDER_KEY",
            }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _agent) = harness
        .post(
            "/agents",
            json!({
                "agent_id": "secret-cli-agent",
                "name": "Secret CLI Agent",
                "cli": {
                    "harness": "copilot-cli",
                    "connection_id": "secret-provider",
                },
            }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, session) = harness
        .post(
            "/sessions",
            json!({
                "agent_id": "secret-cli-agent",
                "interaction_mode": "cli",
            }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{session}");
    let session_id = string_field(&session, "session_id")?;

    let (status, _empty) = harness.delete("/secrets/PROVIDER_KEY/value").await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, error) = harness
        .post(
            &format!("/sessions/{session_id}/terminal/ensure"),
            json!({}),
        )
        .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"], "secret_values_unset");
    assert_eq!(error["missing_secrets"][0]["name"], "PROVIDER_KEY");

    let (status, _empty) = harness
        .put(
            "/secrets/PROVIDER_KEY/value",
            json!({ "value": "restored-key" }),
        )
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, terminal) = harness
        .post(
            &format!("/sessions/{session_id}/terminal/ensure"),
            json!({}),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{terminal}");
    assert_eq!(terminal["state"], "running");

    let creates = harness
        .upstream
        .requests()?
        .into_iter()
        .filter(|request| request.method == "POST" && request.path == "/sessions")
        .collect::<Vec<_>>();
    assert_eq!(creates.len(), 2);
    assert_eq!(
        creates[1].body.as_ref().ok_or("missing recovery body")?["env"]["AGENTSPACE_RUNTIME_RECOVERY"],
        "1"
    );
    assert_eq!(
        creates[1].body.as_ref().ok_or("missing recovery body")?["env"]["CONNECTION_API_KEY"],
        "restored-key"
    );
    Ok(())
}

#[tokio::test]
async fn terminal_routes_validate_mode_missing_origin_and_upstream()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let harness = TestHarness::start().await?;
    let (session_id, _session) = harness.create_cli_session().await?;
    let (status, _chat_agent) = harness
        .post(
            "/agents",
            json!({ "agent_id": "chat-agent", "name": "Chat Agent" }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, chat) = harness
        .post("/sessions", json!({ "agent_id": "chat-agent" }))
        .await?;
    assert_eq!(status, StatusCode::OK);
    let chat_id = string_field(&chat, "session_id")?;

    let (status, _error) = harness
        .get(&format!("/sessions/{chat_id}/terminal"))
        .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _error) = harness.get("/sessions/missing/terminal").await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, message) = harness
        .post(
            &format!("/sessions/{chat_id}/messages"),
            json!({ "message": "hello" }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(message["assistant_message"]["content"], "chat-ok");

    let missing_ws = harness.websocket_url("missing");
    assert_eq!(
        websocket_status(&missing_ws, Some(DENIED_ORIGIN)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        websocket_status(&missing_ws, None).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        websocket_status(&harness.websocket_url(&chat_id), None).await,
        StatusCode::CONFLICT
    );

    harness
        .upstream
        .terminal_unavailable
        .store(true, Ordering::SeqCst);
    assert_eq!(
        websocket_status(&harness.websocket_url(&session_id), None).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    Ok(())
}

#[tokio::test]
async fn websocket_origin_binary_and_sanitized_lifecycle_frames_are_preserved()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let harness = TestHarness::start().await?;
    let (session_id, _session) = harness.create_cli_session().await?;
    let url = harness.websocket_url(&session_id);

    assert_eq!(
        websocket_status(&url, Some(DENIED_ORIGIN)).await,
        StatusCode::FORBIDDEN
    );
    let mut allowed = connect_terminal(&url, Some(ALLOWED_ORIGIN)).await?;
    allowed.close(None).await?;

    let mut socket = connect_terminal(&url, None).await?;
    let ready = socket.next().await.ok_or("missing ready frame")??;
    let ready = match ready {
        ClientMessage::Text(text) => serde_json::from_str::<Value>(text.as_str())?,
        other => return Err(format!("unexpected ready frame: {other:?}").into()),
    };
    assert_eq!(ready["type"], "ready");
    assert!(ready["terminal"].get("socket_path").is_none());
    assert!(ready["terminal"].get("clients").is_none());
    let binary = socket.next().await.ok_or("missing binary frame")??;
    assert_eq!(
        binary,
        ClientMessage::Binary(UPSTREAM_BINARY.to_vec().into())
    );
    let lifecycle = socket.next().await.ok_or("missing lifecycle frame")??;
    let lifecycle = match lifecycle {
        ClientMessage::Text(text) => serde_json::from_str::<Value>(text.as_str())?,
        other => return Err(format!("unexpected lifecycle frame: {other:?}").into()),
    };
    assert_eq!(lifecycle["type"], "exited");
    assert_eq!(lifecycle["terminal"]["attachment_count"], 0);
    assert!(lifecycle["terminal"].get("socket_path").is_none());
    assert!(lifecycle["terminal"].get("pane_pid").is_none());

    let input = vec![0, 0xfe, b'Z', 0x81];
    socket
        .send(ClientMessage::Binary(input.clone().into()))
        .await?;
    assert_eq!(
        socket.next().await.ok_or("missing binary echo")??,
        ClientMessage::Binary(input.clone().into())
    );
    let resize = r#"{"type":"resize","cols":120,"rows":40}"#;
    socket
        .send(ClientMessage::Text(resize.to_owned().into()))
        .await?;
    socket.close(None).await?;

    sleep(Duration::from_millis(25)).await;
    let observations = harness.upstream.observations()?;
    assert!(observations.contains(&WebSocketObservation::Binary(input)));
    assert!(observations.contains(&WebSocketObservation::Text(resize.to_owned())));
    Ok(())
}

#[tokio::test]
async fn websocket_close_codes_propagate_and_upstream_loss_maps_to_4503()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let harness = TestHarness::start().await?;
    let (session_id, _session) = harness.create_cli_session().await?;
    let url = harness.websocket_url(&session_id);

    for code in [1000, 1011, 4404, 4409, 4429, 4503] {
        let reason = format!("close-{code}");
        harness
            .upstream
            .set_websocket_mode(WebSocketMode::CloseOnConnect(code, reason.clone()))?;
        let mut socket = connect_terminal(&url, None).await?;
        let close = socket.next().await.ok_or("missing close frame")??;
        assert_eq!(
            close,
            ClientMessage::Close(Some(ClientCloseFrame {
                code: CloseCode::from(code),
                reason: reason.into(),
            }))
        );
    }

    harness.upstream.set_websocket_mode(WebSocketMode::Normal)?;
    let mut socket = connect_terminal(&url, None).await?;
    for _ in 0..3 {
        let _ = socket.next().await.ok_or("missing initial frame")??;
    }
    socket
        .send(ClientMessage::Close(Some(ClientCloseFrame {
            code: CloseCode::from(4409),
            reason: "browser-close".into(),
        })))
        .await?;
    sleep(Duration::from_millis(25)).await;
    assert!(
        harness
            .upstream
            .observations()?
            .contains(&WebSocketObservation::Close(
                4409,
                "browser-close".to_owned()
            ))
    );

    harness
        .upstream
        .set_websocket_mode(WebSocketMode::DropOnConnect)?;
    let mut socket = connect_terminal(&url, None).await?;
    let error = socket.next().await.ok_or("missing loss lifecycle")??;
    assert!(
        matches!(error, ClientMessage::Text(text) if text.as_str() == r#"{"code":4503,"message":"terminal upstream connection lost","type":"error"}"#)
    );
    let close = socket.next().await.ok_or("missing loss close")??;
    assert_eq!(
        close,
        ClientMessage::Close(Some(ClientCloseFrame {
            code: CloseCode::from(4503),
            reason: "terminal upstream connection lost".into(),
        }))
    );
    Ok(())
}
