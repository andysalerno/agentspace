use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    str::Utf8Error,
    time::Duration,
};

use reqwest::{Method, StatusCode, Url};
use serde_json::{Map, Value, json};

const BASE_URL_ENV: &str = "CLIENT_SERVICE_AGENT_HOST_BASE_URL";
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8001";
const TIMEOUT_ENV: &str = "CLIENT_SERVICE_AGENT_HOST_TIMEOUT";
const DEFAULT_TIMEOUT_SECONDS: &str = "60";

pub type JsonObject = Map<String, Value>;
pub type JsonArray = Vec<JsonObject>;
pub type KernelEvent = JsonObject;
pub type AgentHostResult<T> = Result<T, AgentHostError>;

#[derive(Clone, Debug)]
pub struct AgentHostClient {
    client: reqwest::Client,
    base_url: Url,
    timeout: Duration,
}

impl AgentHostClient {
    pub fn from_env() -> AgentHostResult<Self> {
        let base_url = env::var(BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_BASE_URL.to_owned());
        let raw_timeout =
            env::var(TIMEOUT_ENV).unwrap_or_else(|_| DEFAULT_TIMEOUT_SECONDS.to_owned());
        let timeout_seconds =
            raw_timeout
                .parse::<f64>()
                .map_err(|source| AgentHostError::InvalidTimeout {
                    raw: raw_timeout.clone(),
                    message: source.to_string(),
                })?;
        let timeout = Duration::try_from_secs_f64(timeout_seconds).map_err(|source| {
            AgentHostError::InvalidTimeout {
                raw: raw_timeout,
                message: source.to_string(),
            }
        })?;

        Self::new(&base_url, timeout)
    }

    pub fn new(base_url: &str, timeout: Duration) -> AgentHostResult<Self> {
        let base_url = Url::parse(base_url).map_err(|source| AgentHostError::InvalidBaseUrl {
            raw: base_url.to_owned(),
            message: source.to_string(),
        })?;
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|source| AgentHostError::BuildClient { source })?;

        Ok(Self {
            client,
            base_url,
            timeout,
        })
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    #[must_use]
    pub const fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub async fn create_session(
        &self,
        harness: &str,
        skills: Option<&[String]>,
        env: Option<&BTreeMap<String, String>>,
    ) -> AgentHostResult<JsonObject> {
        let mut payload = JsonObject::new();
        payload.insert("harness".to_owned(), json!(harness));
        if let Some(skills) = skills {
            payload.insert("skills".to_owned(), json!(skills));
        }
        if let Some(env) = env.filter(|env| !env.is_empty()) {
            payload.insert("env".to_owned(), json!(env));
        }

        self.request_object(Method::POST, self.endpoint(&["sessions"])?, Some(payload))
            .await
    }

    pub async fn get_session(&self, session_id: &str) -> AgentHostResult<JsonObject> {
        self.request_object(Method::GET, self.endpoint(&["sessions", session_id])?, None)
            .await
    }

    pub async fn list_sessions(&self, with_stats: bool) -> AgentHostResult<JsonArray> {
        let mut url = self.endpoint(&["sessions"])?;
        if with_stats {
            url.query_pairs_mut().append_pair("with_stats", "true");
        }

        self.request_array(Method::GET, url, None).await
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> AgentHostResult<Vec<KernelEvent>> {
        let mut stream = self.stream_message(session_id, message).await?;
        let mut events = Vec::new();
        while let Some(event) = stream.next_event().await? {
            events.push(event);
        }

        Ok(events)
    }

    pub async fn stream_message(
        &self,
        session_id: &str,
        message: &str,
    ) -> AgentHostResult<AgentHostEventStream> {
        let payload = json!({ "message": message });
        let response = self
            .client
            .post(self.endpoint(&["sessions", session_id, "messages", "stream"])?)
            .json(&payload)
            .send()
            .await
            .map_err(|source| AgentHostError::Http { source })?;
        let response = ensure_success(response).await?;

        Ok(AgentHostEventStream::new(response))
    }

    pub async fn history(&self, session_id: &str) -> AgentHostResult<Vec<Vec<KernelEvent>>> {
        let mut response = self
            .request_object(
                Method::GET,
                self.endpoint(&["sessions", session_id, "history"])?,
                None,
            )
            .await?;
        let raw_history = response
            .remove("history")
            .ok_or(AgentHostError::MissingField { field: "history" })?;

        parse_history(raw_history)
    }

    pub async fn logs(&self, session_id: &str) -> AgentHostResult<Vec<String>> {
        let response = self
            .request_object(
                Method::GET,
                self.endpoint(&["sessions", session_id, "logs"])?,
                None,
            )
            .await?;

        string_list_field(response, "lines")
    }

    pub async fn container_logs(
        &self,
        session_id: &str,
        tail: Option<u64>,
    ) -> AgentHostResult<Vec<String>> {
        let mut url = self.endpoint(&["sessions", session_id, "container-logs"])?;
        if let Some(tail) = tail {
            let tail = tail.to_string();
            url.query_pairs_mut().append_pair("tail", &tail);
        }
        let response = self.request_object(Method::GET, url, None).await?;

        string_list_field(response, "lines")
    }

    pub async fn reset_session(&self, session_id: &str) -> AgentHostResult<JsonObject> {
        self.request_object(
            Method::POST,
            self.endpoint(&["sessions", session_id, "reset"])?,
            None,
        )
        .await
    }

    pub async fn destroy_session(&self, session_id: &str) -> AgentHostResult<()> {
        self.request_empty(Method::DELETE, self.endpoint(&["sessions", session_id])?)
            .await
    }

    pub async fn create_skill(
        &self,
        skill_id: &str,
        files: &BTreeMap<String, String>,
    ) -> AgentHostResult<JsonObject> {
        let payload = json!({ "skill_id": skill_id, "files": files });

        self.request_object(
            Method::POST,
            self.endpoint(&["skills"])?,
            Some(object_from_value(payload)?),
        )
        .await
    }

    pub async fn get_skill(&self, skill_id: &str) -> AgentHostResult<JsonObject> {
        self.request_object(Method::GET, self.endpoint(&["skills", skill_id])?, None)
            .await
    }

    pub async fn list_skills(&self) -> AgentHostResult<JsonArray> {
        self.request_array(Method::GET, self.endpoint(&["skills"])?, None)
            .await
    }

    pub async fn update_skill(
        &self,
        skill_id: &str,
        files: &BTreeMap<String, String>,
    ) -> AgentHostResult<JsonObject> {
        let payload = json!({ "files": files });

        self.request_object(
            Method::PUT,
            self.endpoint(&["skills", skill_id])?,
            Some(object_from_value(payload)?),
        )
        .await
    }

    pub async fn delete_skill(&self, skill_id: &str) -> AgentHostResult<()> {
        self.request_empty(Method::DELETE, self.endpoint(&["skills", skill_id])?)
            .await
    }

    pub async fn info(&self) -> AgentHostResult<JsonObject> {
        self.request_object(Method::GET, self.endpoint(&["info"])?, None)
            .await
    }

    pub async fn create_gateway(
        &self,
        gateway_id: &str,
        gateway_type: &str,
        agent_id: &str,
        env: &BTreeMap<String, String>,
    ) -> AgentHostResult<JsonObject> {
        let payload = json!({
            "gateway_id": gateway_id,
            "gateway_type": gateway_type,
            "agent_id": agent_id,
            "env": env,
        });

        self.request_object(
            Method::POST,
            self.endpoint(&["gateways"])?,
            Some(object_from_value(payload)?),
        )
        .await
    }

    pub async fn list_gateways(&self) -> AgentHostResult<JsonArray> {
        self.request_array(Method::GET, self.endpoint(&["gateways"])?, None)
            .await
    }

    pub async fn get_gateway(&self, gateway_id: &str) -> AgentHostResult<JsonObject> {
        self.request_object(Method::GET, self.endpoint(&["gateways", gateway_id])?, None)
            .await
    }

    pub async fn gateway_logs(&self, gateway_id: &str) -> AgentHostResult<Vec<String>> {
        let response = self
            .request_object(
                Method::GET,
                self.endpoint(&["gateways", gateway_id, "logs"])?,
                None,
            )
            .await?;

        string_list_field(response, "lines")
    }

    pub async fn destroy_gateway(&self, gateway_id: &str) -> AgentHostResult<()> {
        let response = self
            .client
            .delete(self.endpoint(&["gateways", gateway_id])?)
            .send()
            .await
            .map_err(|source| AgentHostError::Http { source })?;
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }

        Err(http_status_error(response).await)
    }

    fn endpoint(&self, segments: &[&str]) -> AgentHostResult<Url> {
        let mut url = self.base_url.clone();
        {
            let mut path_segments =
                url.path_segments_mut()
                    .map_err(|()| AgentHostError::UrlCannotBeBase {
                        base_url: self.base_url.to_string(),
                    })?;
            path_segments.clear();
            path_segments.extend(segments);
        }

        Ok(url)
    }

    async fn request_object(
        &self,
        method: Method,
        url: Url,
        json_payload: Option<JsonObject>,
    ) -> AgentHostResult<JsonObject> {
        let value = self.request_json(method, url, json_payload).await?;

        object_from_value(value)
    }

    async fn request_array(
        &self,
        method: Method,
        url: Url,
        json_payload: Option<JsonObject>,
    ) -> AgentHostResult<JsonArray> {
        let value = self.request_json(method, url, json_payload).await?;

        match value {
            Value::Array(items) => items.into_iter().map(object_from_value).collect(),
            other => Err(AgentHostError::UnexpectedJson {
                expected: "array of objects",
                value: other,
            }),
        }
    }

    async fn request_empty(&self, method: Method, url: Url) -> AgentHostResult<()> {
        let response = self
            .client
            .request(method, url)
            .send()
            .await
            .map_err(|source| AgentHostError::Http { source })?;
        ensure_success(response).await?;

        Ok(())
    }

    async fn request_json(
        &self,
        method: Method,
        url: Url,
        json_payload: Option<JsonObject>,
    ) -> AgentHostResult<Value> {
        let mut request = self.client.request(method, url);
        if let Some(json_payload) = json_payload {
            request = request.json(&json_payload);
        }
        let response = request
            .send()
            .await
            .map_err(|source| AgentHostError::Http { source })?;
        let response = ensure_success(response).await?;

        response
            .json::<Value>()
            .await
            .map_err(|source| AgentHostError::Http { source })
    }
}

#[derive(Debug)]
pub enum AgentHostError {
    InvalidBaseUrl {
        raw: String,
        message: String,
    },
    InvalidTimeout {
        raw: String,
        message: String,
    },
    UrlCannotBeBase {
        base_url: String,
    },
    BuildClient {
        source: reqwest::Error,
    },
    Http {
        source: reqwest::Error,
    },
    HttpStatus {
        status: StatusCode,
        body: String,
    },
    Json {
        source: serde_json::Error,
    },
    Utf8 {
        source: Utf8Error,
    },
    UnexpectedJson {
        expected: &'static str,
        value: Value,
    },
    MissingField {
        field: &'static str,
    },
    InvalidField {
        field: &'static str,
        expected: &'static str,
    },
}

impl Display for AgentHostError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl { raw, message } => {
                write!(formatter, "invalid agent_host base URL {raw:?}: {message}")
            }
            Self::InvalidTimeout { raw, message } => {
                write!(
                    formatter,
                    "invalid {TIMEOUT_ENV} timeout value {raw:?}: {message}"
                )
            }
            Self::UrlCannotBeBase { base_url } => {
                write!(
                    formatter,
                    "agent_host base URL cannot be a base: {base_url}"
                )
            }
            Self::BuildClient { source } => {
                write!(
                    formatter,
                    "failed to build agent_host HTTP client: {source}"
                )
            }
            Self::Http { source } => write!(formatter, "agent_host HTTP error: {source}"),
            Self::HttpStatus { status, body } => {
                write!(formatter, "agent_host returned HTTP {status}: {body}")
            }
            Self::Json { source } => write!(formatter, "agent_host JSON error: {source}"),
            Self::Utf8 { source } => write!(formatter, "agent_host stream was not UTF-8: {source}"),
            Self::UnexpectedJson { expected, value } => {
                write!(
                    formatter,
                    "agent_host returned unexpected JSON; expected {expected}, got {value}"
                )
            }
            Self::MissingField { field } => {
                write!(formatter, "agent_host response is missing field {field:?}")
            }
            Self::InvalidField { field, expected } => {
                write!(
                    formatter,
                    "agent_host response field {field:?} must be {expected}"
                )
            }
        }
    }
}

impl Error for AgentHostError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BuildClient { source } | Self::Http { source } => Some(source),
            Self::Json { source } => Some(source),
            Self::Utf8 { source } => Some(source),
            Self::InvalidBaseUrl { .. }
            | Self::InvalidTimeout { .. }
            | Self::UrlCannotBeBase { .. }
            | Self::HttpStatus { .. }
            | Self::UnexpectedJson { .. }
            | Self::MissingField { .. }
            | Self::InvalidField { .. } => None,
        }
    }
}

pub struct AgentHostEventStream {
    response: reqwest::Response,
    pending: Vec<u8>,
    finished: bool,
}

impl AgentHostEventStream {
    const fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            pending: Vec::new(),
            finished: false,
        }
    }

    pub async fn next_event(&mut self) -> AgentHostResult<Option<KernelEvent>> {
        loop {
            if let Some(line) = self.next_buffered_line() {
                if let Some(event) = parse_stream_line(&line)? {
                    return Ok(Some(event));
                }
                continue;
            }

            if self.finished {
                if self.pending.is_empty() {
                    return Ok(None);
                }
                let line = std::mem::take(&mut self.pending);
                if let Some(event) = parse_stream_line(&line)? {
                    return Ok(Some(event));
                }

                return Ok(None);
            }

            match self
                .response
                .chunk()
                .await
                .map_err(|source| AgentHostError::Http { source })?
            {
                Some(chunk) => self.pending.extend_from_slice(&chunk),
                None => self.finished = true,
            }
        }
    }

    fn next_buffered_line(&mut self) -> Option<Vec<u8>> {
        let newline_index = self.pending.iter().position(|byte| *byte == b'\n')?;
        let mut line = self.pending.drain(..=newline_index).collect::<Vec<_>>();
        if matches!(line.last(), Some(b'\n')) {
            line.pop();
        }
        if matches!(line.last(), Some(b'\r')) {
            line.pop();
        }

        Some(line)
    }
}

async fn ensure_success(response: reqwest::Response) -> AgentHostResult<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }

    Err(http_status_error(response).await)
}

async fn http_status_error(response: reqwest::Response) -> AgentHostError {
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|source| format!("<failed to read error body: {source}>"));

    AgentHostError::HttpStatus { status, body }
}

fn object_from_value(value: Value) -> AgentHostResult<JsonObject> {
    match value {
        Value::Object(object) => Ok(object),
        other => Err(AgentHostError::UnexpectedJson {
            expected: "object",
            value: other,
        }),
    }
}

fn parse_history(raw_history: Value) -> AgentHostResult<Vec<Vec<KernelEvent>>> {
    match raw_history {
        Value::Array(turns) => turns
            .into_iter()
            .map(|turn| match turn {
                Value::Array(events) => events.into_iter().map(object_from_value).collect(),
                other => Err(AgentHostError::UnexpectedJson {
                    expected: "history turn array",
                    value: other,
                }),
            })
            .collect(),
        other => Err(AgentHostError::UnexpectedJson {
            expected: "an array of event arrays",
            value: other,
        }),
    }
}

fn parse_stream_line(line: &[u8]) -> AgentHostResult<Option<KernelEvent>> {
    let text = std::str::from_utf8(line).map_err(|source| AgentHostError::Utf8 { source })?;
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }

    let value =
        serde_json::from_str::<Value>(text).map_err(|source| AgentHostError::Json { source })?;
    match value {
        Value::Object(object) => Ok(Some(object)),
        _other => Ok(None),
    }
}

fn string_list_field(mut object: JsonObject, field: &'static str) -> AgentHostResult<Vec<String>> {
    let value = object
        .remove(field)
        .ok_or(AgentHostError::MissingField { field })?;
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::String(line) => Ok(line),
                _other => Err(AgentHostError::InvalidField {
                    field,
                    expected: "an array of strings",
                }),
            })
            .collect(),
        _other => Err(AgentHostError::InvalidField {
            field,
            expected: "an array of strings",
        }),
    }
}

#[cfg(test)]
#[allow(clippy::similar_names, clippy::too_many_lines)]
mod tests {
    use std::{
        collections::BTreeMap,
        error::Error,
        net::SocketAddr,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{
        Json, Router,
        body::Body,
        extract::{Path, Query, State},
        http::{HeaderMap, Method, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, task::JoinHandle};

    use super::{AgentHostClient, JsonObject};

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecordedRequest {
        method: Method,
        path: String,
        query: Option<String>,
        body: Option<Value>,
    }

    #[derive(Clone, Default)]
    struct TestState {
        requests: Arc<Mutex<Vec<RecordedRequest>>>,
    }

    impl TestState {
        fn record(
            &self,
            method: Method,
            path: impl Into<String>,
            query: Option<String>,
            body: Option<Value>,
        ) -> Result<(), StatusCode> {
            let request = RecordedRequest {
                method,
                path: path.into(),
                query,
                body,
            };
            {
                let mut requests = self
                    .requests
                    .lock()
                    .map_err(|_source| StatusCode::INTERNAL_SERVER_ERROR)?;
                requests.push(request);
            }
            Ok(())
        }

        fn recorded(&self) -> Result<Vec<RecordedRequest>, Box<dyn Error + Send + Sync>> {
            let requests = self
                .requests
                .lock()
                .map_err(|_source| "request recorder mutex poisoned")?;
            Ok(requests.clone())
        }
    }

    struct TestServer {
        base_url: String,
        state: TestState,
        handle: JoinHandle<Result<(), std::io::Error>>,
    }

    impl TestServer {
        async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
            let state = TestState::default();
            let app = test_router(state.clone());
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let handle = tokio::spawn(axum::serve(listener, app).into_future());

            Ok(Self {
                base_url: format_base_url(address),
                state,
                handle,
            })
        }

        fn client(&self) -> Result<AgentHostClient, Box<dyn Error + Send + Sync>> {
            Ok(AgentHostClient::new(
                &self.base_url,
                Duration::from_mins(1),
            )?)
        }

        fn recorded(&self) -> Result<Vec<RecordedRequest>, Box<dyn Error + Send + Sync>> {
            self.state.recorded()
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    fn format_base_url(address: SocketAddr) -> String {
        format!("http://{address}")
    }

    fn test_router(state: TestState) -> Router {
        Router::new()
            .route("/sessions", post(create_session).get(list_sessions))
            .route(
                "/sessions/{session_id}",
                get(get_session).delete(destroy_session),
            )
            .route(
                "/sessions/{session_id}/messages/stream",
                post(stream_message),
            )
            .route("/sessions/{session_id}/history", get(history))
            .route("/sessions/{session_id}/logs", get(logs))
            .route("/sessions/{session_id}/container-logs", get(container_logs))
            .route("/sessions/{session_id}/reset", post(reset_session))
            .route("/skills", post(create_skill).get(list_skills))
            .route(
                "/skills/{skill_id}",
                get(get_skill).put(update_skill).delete(delete_skill),
            )
            .route("/info", get(info))
            .route("/gateways", post(create_gateway).get(list_gateways))
            .route(
                "/gateways/{gateway_id}",
                get(get_gateway).delete(destroy_gateway),
            )
            .route("/gateways/{gateway_id}/logs", get(gateway_logs))
            .with_state(state)
    }

    async fn create_session(
        State(state): State<TestState>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, StatusCode> {
        record_json(&state, Method::POST, "/sessions", None, body.clone())?;
        assert_json_content_type(&headers)?;
        Ok(Json(json!({ "session_id": "session-1", "received": body })))
    }

    async fn list_sessions(
        State(state): State<TestState>,
        Query(query): Query<BTreeMap<String, String>>,
    ) -> Result<Json<Value>, StatusCode> {
        let query_string = query_string(query);
        state.record(Method::GET, "/sessions", query_string, None)?;
        Ok(Json(json!([{ "session_id": "session-1" }])))
    }

    async fn get_session(
        State(state): State<TestState>,
        Path(session_id): Path<String>,
    ) -> Result<Json<Value>, StatusCode> {
        state.record(Method::GET, format!("/sessions/{session_id}"), None, None)?;
        Ok(Json(json!({ "session_id": session_id })))
    }

    async fn destroy_session(
        State(state): State<TestState>,
        Path(session_id): Path<String>,
    ) -> Result<StatusCode, StatusCode> {
        state.record(
            Method::DELETE,
            format!("/sessions/{session_id}"),
            None,
            None,
        )?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn stream_message(
        State(state): State<TestState>,
        Path(session_id): Path<String>,
        Json(body): Json<Value>,
    ) -> Result<Response, StatusCode> {
        record_json(
            &state,
            Method::POST,
            format!("/sessions/{session_id}/messages/stream"),
            None,
            body,
        )?;
        let body = Body::from(
            "\n{\"type\":\"start\"}\n[\"ignored\"]\n{\"type\":\"content\",\"content\":\"hello\"}\n",
        );
        Ok((StatusCode::OK, body).into_response())
    }

    async fn history(
        State(state): State<TestState>,
        Path(session_id): Path<String>,
    ) -> Result<Json<Value>, StatusCode> {
        state.record(
            Method::GET,
            format!("/sessions/{session_id}/history"),
            None,
            None,
        )?;
        Ok(Json(json!({
            "history": [
                [{ "type": "user", "content": "hi" }],
                [{ "type": "assistant", "content": "hello" }]
            ]
        })))
    }

    async fn logs(
        State(state): State<TestState>,
        Path(session_id): Path<String>,
    ) -> Result<Json<Value>, StatusCode> {
        state.record(
            Method::GET,
            format!("/sessions/{session_id}/logs"),
            None,
            None,
        )?;
        Ok(Json(json!({ "lines": ["one", "two"] })))
    }

    async fn container_logs(
        State(state): State<TestState>,
        Path(session_id): Path<String>,
        Query(query): Query<BTreeMap<String, String>>,
    ) -> Result<Json<Value>, StatusCode> {
        state.record(
            Method::GET,
            format!("/sessions/{session_id}/container-logs"),
            query_string(query),
            None,
        )?;
        Ok(Json(json!({ "lines": ["container"] })))
    }

    async fn reset_session(
        State(state): State<TestState>,
        Path(session_id): Path<String>,
    ) -> Result<Json<Value>, StatusCode> {
        state.record(
            Method::POST,
            format!("/sessions/{session_id}/reset"),
            None,
            None,
        )?;
        Ok(Json(json!({ "session_id": session_id, "reset": true })))
    }

    async fn create_skill(
        State(state): State<TestState>,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, StatusCode> {
        record_json(&state, Method::POST, "/skills", None, body.clone())?;
        Ok(Json(
            json!({ "skill_id": body["skill_id"], "files": body["files"] }),
        ))
    }

    async fn list_skills(State(state): State<TestState>) -> Result<Json<Value>, StatusCode> {
        state.record(Method::GET, "/skills", None, None)?;
        Ok(Json(json!([{ "skill_id": "skill-1" }])))
    }

    async fn get_skill(
        State(state): State<TestState>,
        Path(skill_id): Path<String>,
    ) -> Result<Json<Value>, StatusCode> {
        state.record(Method::GET, format!("/skills/{skill_id}"), None, None)?;
        Ok(Json(json!({ "skill_id": skill_id })))
    }

    async fn update_skill(
        State(state): State<TestState>,
        Path(skill_id): Path<String>,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, StatusCode> {
        record_json(
            &state,
            Method::PUT,
            format!("/skills/{skill_id}"),
            None,
            body.clone(),
        )?;
        Ok(Json(
            json!({ "skill_id": skill_id, "files": body["files"] }),
        ))
    }

    async fn delete_skill(
        State(state): State<TestState>,
        Path(skill_id): Path<String>,
    ) -> Result<StatusCode, StatusCode> {
        state.record(Method::DELETE, format!("/skills/{skill_id}"), None, None)?;
        Ok(StatusCode::NO_CONTENT)
    }

    async fn info(State(state): State<TestState>) -> Result<Json<Value>, StatusCode> {
        state.record(Method::GET, "/info", None, None)?;
        Ok(Json(json!({ "service": "agent_host" })))
    }

    async fn create_gateway(
        State(state): State<TestState>,
        Json(body): Json<Value>,
    ) -> Result<Json<Value>, StatusCode> {
        record_json(&state, Method::POST, "/gateways", None, body.clone())?;
        Ok(Json(json!({ "gateway_id": body["gateway_id"] })))
    }

    async fn list_gateways(State(state): State<TestState>) -> Result<Json<Value>, StatusCode> {
        state.record(Method::GET, "/gateways", None, None)?;
        Ok(Json(json!([{ "gateway_id": "gateway-1" }])))
    }

    async fn get_gateway(
        State(state): State<TestState>,
        Path(gateway_id): Path<String>,
    ) -> Result<Json<Value>, StatusCode> {
        state.record(Method::GET, format!("/gateways/{gateway_id}"), None, None)?;
        Ok(Json(json!({ "gateway_id": gateway_id })))
    }

    async fn gateway_logs(
        State(state): State<TestState>,
        Path(gateway_id): Path<String>,
    ) -> Result<Json<Value>, StatusCode> {
        state.record(
            Method::GET,
            format!("/gateways/{gateway_id}/logs"),
            None,
            None,
        )?;
        Ok(Json(json!({ "lines": ["gateway"] })))
    }

    async fn destroy_gateway(
        State(state): State<TestState>,
        Path(gateway_id): Path<String>,
    ) -> Result<StatusCode, StatusCode> {
        state.record(
            Method::DELETE,
            format!("/gateways/{gateway_id}"),
            None,
            None,
        )?;
        Ok(StatusCode::NOT_FOUND)
    }

    fn record_json(
        state: &TestState,
        method: Method,
        path: impl Into<String>,
        query: Option<String>,
        body: Value,
    ) -> Result<(), StatusCode> {
        state.record(method, path, query, Some(body))
    }

    fn assert_json_content_type(headers: &HeaderMap) -> Result<(), StatusCode> {
        let content_type = headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;
        if content_type.starts_with("application/json") {
            Ok(())
        } else {
            Err(StatusCode::BAD_REQUEST)
        }
    }

    fn query_string(query: BTreeMap<String, String>) -> Option<String> {
        if query.is_empty() {
            return None;
        }

        Some(
            query
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("&"),
        )
    }

    fn object(value: Value) -> Result<JsonObject, Box<dyn Error + Send + Sync>> {
        match value {
            Value::Object(object) => Ok(object),
            other => Err(format!("expected object, got {other}").into()),
        }
    }

    #[tokio::test]
    async fn creates_sessions_with_python_payload_shape() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let server = TestServer::start().await?;
        let client = server.client()?;
        let skills = vec!["skill-a".to_owned(), "skill-b".to_owned()];
        let env = BTreeMap::from([("KEY".to_owned(), "value".to_owned())]);

        let response = client
            .create_session("copilot", Some(&skills), Some(&env))
            .await?;

        assert_eq!(response["session_id"], "session-1");
        assert_eq!(
            server.recorded()?,
            vec![RecordedRequest {
                method: Method::POST,
                path: "/sessions".to_owned(),
                query: None,
                body: Some(json!({
                    "harness": "copilot",
                    "skills": ["skill-a", "skill-b"],
                    "env": { "KEY": "value" }
                })),
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn omits_empty_session_options() -> Result<(), Box<dyn Error + Send + Sync>> {
        let server = TestServer::start().await?;
        let client = server.client()?;
        let env = BTreeMap::new();

        client.create_session("copilot", None, Some(&env)).await?;

        assert_eq!(
            server.recorded()?,
            vec![RecordedRequest {
                method: Method::POST,
                path: "/sessions".to_owned(),
                query: None,
                body: Some(json!({ "harness": "copilot" })),
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn sends_query_parameters_only_when_requested() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let server = TestServer::start().await?;
        let client = server.client()?;

        client.list_sessions(false).await?;
        client.list_sessions(true).await?;
        client.container_logs("session-1", None).await?;
        client.container_logs("session-1", Some(25)).await?;

        assert_eq!(
            server.recorded()?,
            vec![
                RecordedRequest {
                    method: Method::GET,
                    path: "/sessions".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::GET,
                    path: "/sessions".to_owned(),
                    query: Some("with_stats=true".to_owned()),
                    body: None,
                },
                RecordedRequest {
                    method: Method::GET,
                    path: "/sessions/session-1/container-logs".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::GET,
                    path: "/sessions/session-1/container-logs".to_owned(),
                    query: Some("tail=25".to_owned()),
                    body: None,
                },
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn parses_streaming_ndjson_events() -> Result<(), Box<dyn Error + Send + Sync>> {
        let server = TestServer::start().await?;
        let client = server.client()?;

        let events = client.send_message("session-1", "hello").await?;

        assert_eq!(
            events,
            vec![
                object(json!({ "type": "start" }))?,
                object(json!({ "type": "content", "content": "hello" }))?,
            ]
        );
        assert_eq!(
            server.recorded()?,
            vec![RecordedRequest {
                method: Method::POST,
                path: "/sessions/session-1/messages/stream".to_owned(),
                query: None,
                body: Some(json!({ "message": "hello" })),
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn maps_history_and_line_responses() -> Result<(), Box<dyn Error + Send + Sync>> {
        let server = TestServer::start().await?;
        let client = server.client()?;

        let history = client.history("session-1").await?;
        let logs = client.logs("session-1").await?;
        let gateway_logs = client.gateway_logs("gateway-1").await?;

        assert_eq!(
            history,
            vec![
                vec![object(json!({ "type": "user", "content": "hi" }))?],
                vec![object(json!({ "type": "assistant", "content": "hello" }))?],
            ]
        );
        assert_eq!(logs, vec!["one".to_owned(), "two".to_owned()]);
        assert_eq!(gateway_logs, vec!["gateway".to_owned()]);

        Ok(())
    }

    #[tokio::test]
    async fn covers_skill_gateway_and_delete_methods() -> Result<(), Box<dyn Error + Send + Sync>> {
        let server = TestServer::start().await?;
        let client = server.client()?;
        let files = BTreeMap::from([("SKILL.md".to_owned(), "content".to_owned())]);
        let gateway_env = BTreeMap::from([("TOKEN".to_owned(), "redacted".to_owned())]);

        client.get_session("session-1").await?;
        client.reset_session("session-1").await?;
        client.destroy_session("session-1").await?;
        client.create_skill("skill-1", &files).await?;
        client.get_skill("skill-1").await?;
        client.list_skills().await?;
        client.update_skill("skill-1", &files).await?;
        client.delete_skill("skill-1").await?;
        client.info().await?;
        client
            .create_gateway("gateway-1", "stdio", "agent-1", &gateway_env)
            .await?;
        client.list_gateways().await?;
        client.get_gateway("gateway-1").await?;
        client.destroy_gateway("gateway-1").await?;

        assert_eq!(
            server.recorded()?,
            vec![
                RecordedRequest {
                    method: Method::GET,
                    path: "/sessions/session-1".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::POST,
                    path: "/sessions/session-1/reset".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::DELETE,
                    path: "/sessions/session-1".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::POST,
                    path: "/skills".to_owned(),
                    query: None,
                    body: Some(json!({
                        "skill_id": "skill-1",
                        "files": { "SKILL.md": "content" }
                    })),
                },
                RecordedRequest {
                    method: Method::GET,
                    path: "/skills/skill-1".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::GET,
                    path: "/skills".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::PUT,
                    path: "/skills/skill-1".to_owned(),
                    query: None,
                    body: Some(json!({ "files": { "SKILL.md": "content" } })),
                },
                RecordedRequest {
                    method: Method::DELETE,
                    path: "/skills/skill-1".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::GET,
                    path: "/info".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::POST,
                    path: "/gateways".to_owned(),
                    query: None,
                    body: Some(json!({
                        "gateway_id": "gateway-1",
                        "gateway_type": "stdio",
                        "agent_id": "agent-1",
                        "env": { "TOKEN": "redacted" }
                    })),
                },
                RecordedRequest {
                    method: Method::GET,
                    path: "/gateways".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::GET,
                    path: "/gateways/gateway-1".to_owned(),
                    query: None,
                    body: None,
                },
                RecordedRequest {
                    method: Method::DELETE,
                    path: "/gateways/gateway-1".to_owned(),
                    query: None,
                    body: None,
                },
            ]
        );

        Ok(())
    }
}
