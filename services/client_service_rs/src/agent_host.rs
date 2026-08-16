use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    str::Utf8Error,
    time::{Duration, Instant},
};

use reqwest::{Method, StatusCode, Url, header};
use serde_json::{Map, Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Error as WebSocketError, protocol::WebSocketConfig},
};

use crate::models::WorkspaceMountRecord;

const BASE_URL_ENV: &str = "CLIENT_SERVICE_AGENT_HOST_BASE_URL";
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8001";
const TIMEOUT_ENV: &str = "CLIENT_SERVICE_AGENT_HOST_TIMEOUT";
const DEFAULT_TIMEOUT_SECONDS: &str = "60";

pub type JsonObject = Map<String, Value>;
pub type JsonArray = Vec<JsonObject>;
pub type KernelEvent = JsonObject;
pub type AgentHostResult<T> = Result<T, AgentHostError>;
pub type AgentHostWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const MAX_TERMINAL_WEBSOCKET_MESSAGE_SIZE: usize = 1024 * 1024;
const MAX_TERMINAL_WEBSOCKET_WRITE_BUFFER_SIZE: usize = 1024 * 1024;

pub struct AgentHostSessionCreate<'a> {
    pub session_id: &'a str,
    pub telemetry_volume_identity: Option<&'a str>,
    pub interaction_mode: &'a str,
    pub harness: &'a str,
    pub skills: Option<&'a [String]>,
    pub env: Option<&'a BTreeMap<String, String>>,
    pub additional_paths: Option<&'a [String]>,
    pub workspace_mounts: Option<&'a [WorkspaceMountRecord]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentHostDownload {
    pub content_type: String,
    pub content_disposition: String,
    pub body: Vec<u8>,
}

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
        request: AgentHostSessionCreate<'_>,
    ) -> AgentHostResult<JsonObject> {
        let mut payload = JsonObject::new();
        payload.insert("session_id".to_owned(), json!(request.session_id));
        if let Some(telemetry_volume_identity) = request.telemetry_volume_identity {
            payload.insert(
                "telemetry_volume_identity".to_owned(),
                json!(telemetry_volume_identity),
            );
        }
        payload.insert(
            "interaction_mode".to_owned(),
            json!(request.interaction_mode),
        );
        payload.insert("harness".to_owned(), json!(request.harness));
        if let Some(skills) = request.skills {
            payload.insert("skills".to_owned(), json!(skills));
        }
        if let Some(env) = request.env.filter(|env| !env.is_empty()) {
            payload.insert("env".to_owned(), json!(env));
        }
        if let Some(additional_paths) = request.additional_paths.filter(|paths| !paths.is_empty()) {
            payload.insert("additional_paths".to_owned(), json!(additional_paths));
        }
        if let Some(workspace_mounts) = request.workspace_mounts.filter(|mounts| !mounts.is_empty())
        {
            payload.insert("workspace_mounts".to_owned(), json!(workspace_mounts));
        }

        self.request_object(Method::POST, self.endpoint(&["sessions"])?, Some(payload))
            .await
    }

    pub async fn terminal_status(&self, session_id: &str) -> AgentHostResult<JsonObject> {
        self.request_object(
            Method::GET,
            self.endpoint(&["sessions", session_id, "terminal"])?,
            None,
        )
        .await
    }

    pub async fn terminal_ensure(&self, session_id: &str) -> AgentHostResult<JsonObject> {
        self.terminal_control(session_id, "ensure", None).await
    }

    pub async fn terminal_stop(&self, session_id: &str) -> AgentHostResult<JsonObject> {
        self.terminal_control(session_id, "stop", None).await
    }

    pub async fn terminal_resume(&self, session_id: &str) -> AgentHostResult<JsonObject> {
        self.terminal_control(session_id, "resume", None).await
    }

    pub fn terminal_websocket_url(&self, session_id: &str) -> AgentHostResult<Url> {
        let mut url = self.endpoint(&["sessions", session_id, "terminal", "ws"])?;
        let websocket_scheme = match url.scheme() {
            "http" => "ws",
            "https" => "wss",
            scheme => {
                return Err(AgentHostError::InvalidWebSocketScheme {
                    scheme: scheme.to_owned(),
                });
            }
        };
        url.set_scheme(websocket_scheme)
            .map_err(|()| AgentHostError::InvalidWebSocketScheme {
                scheme: websocket_scheme.to_owned(),
            })?;
        Ok(url)
    }

    pub async fn connect_terminal_websocket(
        &self,
        session_id: &str,
    ) -> AgentHostResult<AgentHostWebSocket> {
        let url = self.terminal_websocket_url(session_id)?;
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(MAX_TERMINAL_WEBSOCKET_MESSAGE_SIZE);
        config.max_frame_size = Some(MAX_TERMINAL_WEBSOCKET_MESSAGE_SIZE);
        config.max_write_buffer_size = MAX_TERMINAL_WEBSOCKET_WRITE_BUFFER_SIZE;
        match tokio::time::timeout(
            self.timeout,
            connect_async_with_config(url.as_str(), Some(config), true),
        )
        .await
        {
            Ok(Ok((websocket, _response))) => Ok(websocket),
            Ok(Err(source)) => Err(AgentHostError::WebSocket { source }),
            Err(_elapsed) => Err(AgentHostError::WebSocketTimeout {
                timeout: self.timeout,
            }),
        }
    }

    async fn terminal_control(
        &self,
        session_id: &str,
        action: &str,
        payload: Option<JsonObject>,
    ) -> AgentHostResult<JsonObject> {
        self.request_object(
            Method::POST,
            self.endpoint(&["sessions", session_id, "terminal", action])?,
            payload,
        )
        .await
    }

    pub async fn cleanup_runtime(
        &self,
        owned_session_ids: &[String],
        dry_run: bool,
        reviewed_resources: Option<&[Value]>,
    ) -> AgentHostResult<JsonObject> {
        let mut payload = JsonObject::new();
        payload.insert("owned_session_ids".to_owned(), json!(owned_session_ids));
        payload.insert("dry_run".to_owned(), json!(dry_run));
        if let Some(reviewed_resources) = reviewed_resources {
            payload.insert("reviewed_resources".to_owned(), json!(reviewed_resources));
        }
        self.request_object(
            Method::POST,
            self.endpoint(&["management", "runtime-cleanup"])?,
            Some(payload),
        )
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
        let mut payload = JsonObject::new();
        payload.insert("message".to_owned(), json!(message));
        let method = Method::POST;
        let url = self.endpoint(&["sessions", session_id, "messages", "stream"])?;
        let trace_context = RequestTraceContext::from_url_and_payload(&url, Some(&payload));
        let payload_trace = JsonPayloadTrace::from_payload(Some(&payload));
        let started_at = Instant::now();
        log_agent_host_request_start(&method, &trace_context, &payload_trace);
        let response = match tokio::time::timeout(
            self.timeout,
            self.client
                .request(method.clone(), url)
                .json(&payload)
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(source)) => {
                log_agent_host_request_send_error(
                    &method,
                    &trace_context,
                    &payload_trace,
                    &source,
                    started_at.elapsed(),
                );
                return Err(AgentHostError::Http { source });
            }
            Err(_elapsed) => {
                let error = AgentHostError::InitialResponseTimeout {
                    timeout: self.timeout,
                };
                log_agent_host_request_failure(
                    &method,
                    &trace_context,
                    &payload_trace,
                    StatusCode::REQUEST_TIMEOUT,
                    started_at.elapsed(),
                    &error,
                );
                return Err(error);
            }
        };
        let status = response.status();
        let response = match ensure_success(response).await {
            Ok(response) => response,
            Err(error) => {
                log_agent_host_request_failure(
                    &method,
                    &trace_context,
                    &payload_trace,
                    status,
                    started_at.elapsed(),
                    &error,
                );
                return Err(error);
            }
        };
        log_agent_host_request_success(
            &method,
            &trace_context,
            &payload_trace,
            status,
            started_at.elapsed(),
            JsonResponseTrace::stream(),
        );

        Ok(AgentHostEventStream::new(response, trace_context, status))
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

    pub async fn snapshot_session_workspace(
        &self,
        session_id: &str,
        workspace_id: &str,
        volume_name: &str,
        exclude_paths: &[String],
    ) -> AgentHostResult<JsonObject> {
        let mut payload = JsonObject::new();
        payload.insert("workspace_id".to_owned(), json!(workspace_id));
        payload.insert("volume_name".to_owned(), json!(volume_name));
        payload.insert("exclude_paths".to_owned(), json!(exclude_paths));
        self.request_object(
            Method::POST,
            self.endpoint(&["sessions", session_id, "workspace", "snapshot"])?,
            Some(payload),
        )
        .await
    }

    pub async fn clone_workspace(
        &self,
        source_volume_name: &str,
        target_workspace_id: &str,
        target_volume_name: &str,
    ) -> AgentHostResult<JsonObject> {
        let mut payload = JsonObject::new();
        payload.insert("source_volume_name".to_owned(), json!(source_volume_name));
        payload.insert("target_workspace_id".to_owned(), json!(target_workspace_id));
        payload.insert("target_volume_name".to_owned(), json!(target_volume_name));
        self.request_object(
            Method::POST,
            self.endpoint(&["workspaces", "clone"])?,
            Some(payload),
        )
        .await
    }

    pub async fn open_workspace_vscode(
        &self,
        workspace_id: &str,
        volume_name: &str,
    ) -> AgentHostResult<JsonObject> {
        let mut payload = JsonObject::new();
        payload.insert("workspace_id".to_owned(), json!(workspace_id));
        payload.insert("volume_name".to_owned(), json!(volume_name));
        self.request_object(
            Method::POST,
            self.endpoint(&["workspaces", "vscode"])?,
            Some(payload),
        )
        .await
    }

    pub async fn destroy_session(&self, session_id: &str) -> AgentHostResult<()> {
        let method = Method::DELETE;
        let url = self.endpoint(&["sessions", session_id])?;
        let trace_context = RequestTraceContext::from_url_and_payload(&url, None);
        let payload_trace = JsonPayloadTrace::from_payload(None);
        let started_at = Instant::now();
        log_agent_host_request_start(&method, &trace_context, &payload_trace);
        let response = match self
            .client
            .request(method.clone(), url)
            .timeout(self.timeout)
            .send()
            .await
        {
            Ok(response) => response,
            Err(source) => {
                log_agent_host_request_send_error(
                    &method,
                    &trace_context,
                    &payload_trace,
                    &source,
                    started_at.elapsed(),
                );
                return Err(AgentHostError::Http { source });
            }
        };
        let status = response.status();
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            log_agent_host_request_success(
                &method,
                &trace_context,
                &payload_trace,
                status,
                started_at.elapsed(),
                JsonResponseTrace::empty(),
            );
            return Ok(());
        }

        let error = http_status_error(response).await;
        log_agent_host_request_failure(
            &method,
            &trace_context,
            &payload_trace,
            status,
            started_at.elapsed(),
            &error,
        );
        Err(error)
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

    pub async fn download_skill(&self, skill_id: &str) -> AgentHostResult<AgentHostDownload> {
        self.request_binary(
            Method::GET,
            self.endpoint(&["skills", skill_id, "download"])?,
        )
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

    pub async fn list_skill_versions(&self, skill_id: &str) -> AgentHostResult<JsonArray> {
        self.request_array(
            Method::GET,
            self.endpoint(&["skills", skill_id, "versions"])?,
            None,
        )
        .await
    }

    pub async fn rollback_skill_version(
        &self,
        skill_id: &str,
        version: u64,
    ) -> AgentHostResult<JsonObject> {
        let version = version.to_string();
        self.request_object(
            Method::POST,
            self.endpoint(&["skills", skill_id, "versions", &version, "rollback"])?,
            None,
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
        let method = Method::DELETE;
        let url = self.endpoint(&["gateways", gateway_id])?;
        let trace_context = RequestTraceContext::from_url_and_payload(&url, None);
        let payload_trace = JsonPayloadTrace::from_payload(None);
        let started_at = Instant::now();
        log_agent_host_request_start(&method, &trace_context, &payload_trace);
        let response = match self
            .client
            .request(method.clone(), url)
            .timeout(self.timeout)
            .send()
            .await
        {
            Ok(response) => response,
            Err(source) => {
                log_agent_host_request_send_error(
                    &method,
                    &trace_context,
                    &payload_trace,
                    &source,
                    started_at.elapsed(),
                );
                return Err(AgentHostError::Http { source });
            }
        };
        let status = response.status();
        if response.status().is_success() || response.status() == StatusCode::NOT_FOUND {
            log_agent_host_request_success(
                &method,
                &trace_context,
                &payload_trace,
                status,
                started_at.elapsed(),
                JsonResponseTrace::empty(),
            );
            return Ok(());
        }

        let error = http_status_error(response).await;
        log_agent_host_request_failure(
            &method,
            &trace_context,
            &payload_trace,
            status,
            started_at.elapsed(),
            &error,
        );
        Err(error)
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
        let trace_context = RequestTraceContext::from_url_and_payload(&url, json_payload.as_ref());
        let value = self.request_json(method, url, json_payload).await?;

        match object_from_value(value) {
            Ok(object) => Ok(object),
            Err(error) => {
                log_agent_host_response_shape_error(&trace_context, "object", &error);
                Err(error)
            }
        }
    }

    async fn request_array(
        &self,
        method: Method,
        url: Url,
        json_payload: Option<JsonObject>,
    ) -> AgentHostResult<JsonArray> {
        let trace_context = RequestTraceContext::from_url_and_payload(&url, json_payload.as_ref());
        let value = self.request_json(method, url, json_payload).await?;

        match value {
            Value::Array(items) => items.into_iter().map(object_from_value).collect(),
            other => {
                let error = AgentHostError::UnexpectedJson {
                    expected: "array of objects",
                    value: other,
                };
                log_agent_host_response_shape_error(&trace_context, "array of objects", &error);
                Err(error)
            }
        }
    }

    async fn request_empty(&self, method: Method, url: Url) -> AgentHostResult<()> {
        let trace_context = RequestTraceContext::from_url_and_payload(&url, None);
        let payload_trace = JsonPayloadTrace::from_payload(None);
        let started_at = Instant::now();
        log_agent_host_request_start(&method, &trace_context, &payload_trace);
        let response = match self
            .client
            .request(method.clone(), url)
            .timeout(self.timeout)
            .send()
            .await
        {
            Ok(response) => response,
            Err(source) => {
                log_agent_host_request_send_error(
                    &method,
                    &trace_context,
                    &payload_trace,
                    &source,
                    started_at.elapsed(),
                );
                return Err(AgentHostError::Http { source });
            }
        };
        let status = response.status();
        match ensure_success(response).await {
            Ok(_response) => {
                log_agent_host_request_success(
                    &method,
                    &trace_context,
                    &payload_trace,
                    status,
                    started_at.elapsed(),
                    JsonResponseTrace::empty(),
                );
            }
            Err(error) => {
                log_agent_host_request_failure(
                    &method,
                    &trace_context,
                    &payload_trace,
                    status,
                    started_at.elapsed(),
                    &error,
                );
                return Err(error);
            }
        }

        Ok(())
    }

    async fn request_json(
        &self,
        method: Method,
        url: Url,
        json_payload: Option<JsonObject>,
    ) -> AgentHostResult<Value> {
        let trace_context = RequestTraceContext::from_url_and_payload(&url, json_payload.as_ref());
        let payload_trace = JsonPayloadTrace::from_payload(json_payload.as_ref());
        let started_at = Instant::now();
        log_agent_host_request_start(&method, &trace_context, &payload_trace);
        let mut request = self.client.request(method.clone(), url);
        if let Some(json_payload) = json_payload {
            request = request.json(&json_payload);
        }
        let response = match request.timeout(self.timeout).send().await {
            Ok(response) => response,
            Err(source) => {
                log_agent_host_request_send_error(
                    &method,
                    &trace_context,
                    &payload_trace,
                    &source,
                    started_at.elapsed(),
                );
                return Err(AgentHostError::Http { source });
            }
        };
        let status = response.status();
        let response = match ensure_success(response).await {
            Ok(response) => response,
            Err(error) => {
                log_agent_host_request_failure(
                    &method,
                    &trace_context,
                    &payload_trace,
                    status,
                    started_at.elapsed(),
                    &error,
                );
                return Err(error);
            }
        };

        match response.json::<Value>().await {
            Ok(value) => {
                log_agent_host_request_success(
                    &method,
                    &trace_context,
                    &payload_trace,
                    status,
                    started_at.elapsed(),
                    JsonResponseTrace::from_value(&value),
                );
                Ok(value)
            }
            Err(source) => {
                log_agent_host_response_read_error(
                    &method,
                    &trace_context,
                    &payload_trace,
                    status,
                    started_at.elapsed(),
                    "response_json_read",
                    &source,
                );
                Err(AgentHostError::Http { source })
            }
        }
    }

    async fn request_binary(&self, method: Method, url: Url) -> AgentHostResult<AgentHostDownload> {
        let trace_context = RequestTraceContext::from_url_and_payload(&url, None);
        let payload_trace = JsonPayloadTrace::from_payload(None);
        let started_at = Instant::now();
        log_agent_host_request_start(&method, &trace_context, &payload_trace);
        let response = match self
            .client
            .request(method.clone(), url)
            .timeout(self.timeout)
            .send()
            .await
        {
            Ok(response) => response,
            Err(source) => {
                log_agent_host_request_send_error(
                    &method,
                    &trace_context,
                    &payload_trace,
                    &source,
                    started_at.elapsed(),
                );
                return Err(AgentHostError::Http { source });
            }
        };
        let status = response.status();
        let response = match ensure_success(response).await {
            Ok(response) => response,
            Err(error) => {
                log_agent_host_request_failure(
                    &method,
                    &trace_context,
                    &payload_trace,
                    status,
                    started_at.elapsed(),
                    &error,
                );
                return Err(error);
            }
        };
        let content_type =
            required_header(response.headers(), &header::CONTENT_TYPE, "content-type")?;
        let content_disposition = required_header(
            response.headers(),
            &header::CONTENT_DISPOSITION,
            "content-disposition",
        )?;
        let body = match response.bytes().await {
            Ok(bytes) => bytes.to_vec(),
            Err(source) => {
                log_agent_host_response_read_error(
                    &method,
                    &trace_context,
                    &payload_trace,
                    status,
                    started_at.elapsed(),
                    "response_body_read",
                    &source,
                );
                return Err(AgentHostError::Http { source });
            }
        };
        log_agent_host_request_success(
            &method,
            &trace_context,
            &payload_trace,
            status,
            started_at.elapsed(),
            JsonResponseTrace::binary(body.len()),
        );
        Ok(AgentHostDownload {
            content_type,
            content_disposition,
            body,
        })
    }
}

#[derive(Clone, Debug, Default)]
struct RequestTraceContext {
    target: String,
    session_id: Option<String>,
    gateway_id: Option<String>,
    skill_id: Option<String>,
}

impl RequestTraceContext {
    fn from_url_and_payload(url: &Url, payload: Option<&JsonObject>) -> Self {
        let mut context = Self {
            target: request_target(url),
            ..Self::default()
        };
        let segments = url
            .path_segments()
            .map_or_else(Vec::new, Iterator::collect::<Vec<_>>);
        match segments.as_slice() {
            ["sessions", session_id, ..] => context.session_id = Some((*session_id).to_owned()),
            ["gateways", gateway_id, ..] => context.gateway_id = Some((*gateway_id).to_owned()),
            ["skills", skill_id, ..] => context.skill_id = Some((*skill_id).to_owned()),
            _other => {}
        }
        if let Some(payload) = payload {
            context.add_payload_ids(payload);
        }

        context
    }

    fn add_payload_ids(&mut self, payload: &JsonObject) {
        if self.session_id.is_none() {
            self.session_id = payload
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if self.gateway_id.is_none() {
            self.gateway_id = payload
                .get("gateway_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if self.skill_id.is_none() {
            self.skill_id = payload
                .get("skill_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }

    fn session_id(&self) -> &str {
        self.session_id.as_deref().unwrap_or("")
    }

    fn gateway_id(&self) -> &str {
        self.gateway_id.as_deref().unwrap_or("")
    }

    fn skill_id(&self) -> &str {
        self.skill_id.as_deref().unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct JsonPayloadTrace {
    present: bool,
    fields: usize,
    skills: usize,
    env: usize,
    files: usize,
}

impl JsonPayloadTrace {
    fn from_payload(payload: Option<&JsonObject>) -> Self {
        let Some(payload) = payload else {
            return Self::default();
        };

        Self {
            present: true,
            fields: payload.len(),
            skills: object_array_len(payload, "skills"),
            env: object_object_len(payload, "env"),
            files: object_object_len(payload, "files"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct JsonResponseTrace {
    kind: &'static str,
    fields: usize,
    items: usize,
}

impl JsonResponseTrace {
    const fn empty() -> Self {
        Self {
            kind: "empty",
            fields: 0,
            items: 0,
        }
    }

    const fn stream() -> Self {
        Self {
            kind: "stream",
            fields: 0,
            items: 0,
        }
    }

    const fn binary(bytes: usize) -> Self {
        Self {
            kind: "binary",
            fields: 0,
            items: bytes,
        }
    }

    fn from_value(value: &Value) -> Self {
        match value {
            Value::Object(object) => Self {
                kind: "object",
                fields: object.len(),
                items: 0,
            },
            Value::Array(items) => Self {
                kind: "array",
                fields: 0,
                items: items.len(),
            },
            Value::Null => Self {
                kind: "null",
                fields: 0,
                items: 0,
            },
            Value::Bool(_value) => Self {
                kind: "bool",
                fields: 0,
                items: 0,
            },
            Value::Number(_value) => Self {
                kind: "number",
                fields: 0,
                items: 0,
            },
            Value::String(_value) => Self {
                kind: "string",
                fields: 0,
                items: 0,
            },
        }
    }
}

fn request_target(url: &Url) -> String {
    let mut target = url.path().to_owned();
    let mut first_query_pair = true;
    for (key, _value) in url.query_pairs() {
        if first_query_pair {
            target.push('?');
            first_query_pair = false;
        } else {
            target.push('&');
        }
        target.push_str(&key);
        target.push_str("=<redacted>");
    }

    target
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn object_array_len(object: &JsonObject, field: &str) -> usize {
    object
        .get(field)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn object_object_len(object: &JsonObject, field: &str) -> usize {
    object
        .get(field)
        .and_then(Value::as_object)
        .map_or(0, Map::len)
}

fn required_header(
    headers: &header::HeaderMap,
    name: &header::HeaderName,
    header_name: &'static str,
) -> AgentHostResult<String> {
    let value = headers
        .get(name)
        .ok_or_else(|| AgentHostError::MissingHeader {
            header: header_name,
        })?;
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|source| AgentHostError::Header {
            header: header_name,
            source,
        })
}

fn log_agent_host_request_start(
    method: &Method,
    context: &RequestTraceContext,
    payload: &JsonPayloadTrace,
) {
    tracing::debug!(
        method = %method,
        target = %context.target,
        session_id = context.session_id(),
        gateway_id = context.gateway_id(),
        skill_id = context.skill_id(),
        payload_present = payload.present,
        payload_fields = payload.fields,
        payload_skills = payload.skills,
        payload_env = payload.env,
        payload_files = payload.files,
        "agent_host request started"
    );
}

fn log_agent_host_request_send_error(
    method: &Method,
    context: &RequestTraceContext,
    payload: &JsonPayloadTrace,
    source: &reqwest::Error,
    elapsed: Duration,
) {
    tracing::warn!(
        method = %method,
        target = %context.target,
        session_id = context.session_id(),
        gateway_id = context.gateway_id(),
        skill_id = context.skill_id(),
        elapsed_ms = duration_ms(elapsed),
        payload_present = payload.present,
        payload_fields = payload.fields,
        payload_skills = payload.skills,
        payload_env = payload.env,
        payload_files = payload.files,
        error_kind = reqwest_error_kind(source),
        error_status = reqwest_error_status(source),
        "agent_host request send failed"
    );
}

fn log_agent_host_request_failure(
    method: &Method,
    context: &RequestTraceContext,
    payload: &JsonPayloadTrace,
    status: StatusCode,
    elapsed: Duration,
    error: &AgentHostError,
) {
    tracing::warn!(
        method = %method,
        target = %context.target,
        session_id = context.session_id(),
        gateway_id = context.gateway_id(),
        skill_id = context.skill_id(),
        status = status.as_u16(),
        elapsed_ms = duration_ms(elapsed),
        payload_present = payload.present,
        payload_fields = payload.fields,
        payload_skills = payload.skills,
        payload_env = payload.env,
        payload_files = payload.files,
        error_kind = agent_host_error_kind(error),
        error_status = agent_host_error_status(error),
        "agent_host request failed"
    );
}

fn log_agent_host_request_success(
    method: &Method,
    context: &RequestTraceContext,
    payload: &JsonPayloadTrace,
    status: StatusCode,
    elapsed: Duration,
    response: JsonResponseTrace,
) {
    tracing::info!(
        method = %method,
        target = %context.target,
        session_id = context.session_id(),
        gateway_id = context.gateway_id(),
        skill_id = context.skill_id(),
        status = status.as_u16(),
        elapsed_ms = duration_ms(elapsed),
        payload_present = payload.present,
        payload_fields = payload.fields,
        payload_skills = payload.skills,
        payload_env = payload.env,
        payload_files = payload.files,
        response_kind = response.kind,
        response_fields = response.fields,
        response_items = response.items,
        "agent_host request completed"
    );
}

fn log_agent_host_response_read_error(
    method: &Method,
    context: &RequestTraceContext,
    payload: &JsonPayloadTrace,
    status: StatusCode,
    elapsed: Duration,
    error_kind: &'static str,
    source: &reqwest::Error,
) {
    tracing::warn!(
        method = %method,
        target = %context.target,
        session_id = context.session_id(),
        gateway_id = context.gateway_id(),
        skill_id = context.skill_id(),
        status = status.as_u16(),
        elapsed_ms = duration_ms(elapsed),
        payload_present = payload.present,
        payload_fields = payload.fields,
        payload_skills = payload.skills,
        payload_env = payload.env,
        payload_files = payload.files,
        error_kind,
        source_kind = reqwest_error_kind(source),
        error_status = reqwest_error_status(source),
        "agent_host response read failed"
    );
}

fn log_agent_host_response_shape_error(
    context: &RequestTraceContext,
    expected: &'static str,
    error: &AgentHostError,
) {
    tracing::warn!(
        target = %context.target,
        session_id = context.session_id(),
        gateway_id = context.gateway_id(),
        skill_id = context.skill_id(),
        expected = expected,
        error_kind = agent_host_error_kind(error),
        error_status = agent_host_error_status(error),
        "agent_host response shape mismatch"
    );
}

fn reqwest_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_decode() {
        "decode"
    } else if error.is_body() {
        "body"
    } else if error.is_request() {
        "request"
    } else {
        "http"
    }
}

fn reqwest_error_status(error: &reqwest::Error) -> u16 {
    error.status().map_or(0, |status| status.as_u16())
}

fn agent_host_error_kind(error: &AgentHostError) -> &'static str {
    match error {
        AgentHostError::InvalidBaseUrl { .. } => "invalid_base_url",
        AgentHostError::InvalidTimeout { .. } => "invalid_timeout",
        AgentHostError::InvalidWebSocketScheme { .. } => "invalid_websocket_scheme",
        AgentHostError::InitialResponseTimeout { .. } => "initial_response_timeout",
        AgentHostError::WebSocketTimeout { .. } => "websocket_timeout",
        AgentHostError::UrlCannotBeBase { .. } => "url_cannot_be_base",
        AgentHostError::BuildClient { .. } => "build_client",
        AgentHostError::Http { source } => reqwest_error_kind(source),
        AgentHostError::WebSocket { .. } => "websocket",
        AgentHostError::HttpStatus { .. } => "http_status",
        AgentHostError::Json { .. } => "json",
        AgentHostError::Header { .. } => "header",
        AgentHostError::Utf8 { .. } => "utf8",
        AgentHostError::UnexpectedJson { .. } => "unexpected_json",
        AgentHostError::MissingHeader { .. } => "missing_header",
        AgentHostError::MissingField { .. } => "missing_field",
        AgentHostError::InvalidField { .. } => "invalid_field",
    }
}

fn agent_host_error_status(error: &AgentHostError) -> u16 {
    match error {
        AgentHostError::Http { source } => reqwest_error_status(source),
        AgentHostError::WebSocket {
            source: WebSocketError::Http(response),
        } => response.status().as_u16(),
        AgentHostError::HttpStatus { status, .. } => status.as_u16(),
        AgentHostError::WebSocket { .. }
        | AgentHostError::InvalidBaseUrl { .. }
        | AgentHostError::InvalidTimeout { .. }
        | AgentHostError::InvalidWebSocketScheme { .. }
        | AgentHostError::InitialResponseTimeout { .. }
        | AgentHostError::WebSocketTimeout { .. }
        | AgentHostError::UrlCannotBeBase { .. }
        | AgentHostError::BuildClient { .. }
        | AgentHostError::Json { .. }
        | AgentHostError::Header { .. }
        | AgentHostError::Utf8 { .. }
        | AgentHostError::UnexpectedJson { .. }
        | AgentHostError::MissingHeader { .. }
        | AgentHostError::MissingField { .. }
        | AgentHostError::InvalidField { .. } => 0,
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
    InvalidWebSocketScheme {
        scheme: String,
    },
    InitialResponseTimeout {
        timeout: Duration,
    },
    WebSocketTimeout {
        timeout: Duration,
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
    WebSocket {
        source: WebSocketError,
    },
    HttpStatus {
        status: StatusCode,
        body: String,
    },
    Json {
        source: serde_json::Error,
    },
    Header {
        header: &'static str,
        source: header::ToStrError,
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
    MissingHeader {
        header: &'static str,
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
            Self::InvalidWebSocketScheme { scheme } => {
                write!(
                    formatter,
                    "agent_host WebSocket requires an HTTP(S) base URL, got scheme {scheme:?}"
                )
            }
            Self::InitialResponseTimeout { timeout } => {
                write!(
                    formatter,
                    "agent_host stream did not start within {timeout:?}"
                )
            }
            Self::WebSocketTimeout { timeout } => {
                write!(
                    formatter,
                    "agent_host terminal WebSocket did not connect within {timeout:?}"
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
            Self::WebSocket { source } => {
                write!(formatter, "agent_host WebSocket error: {source}")
            }
            Self::HttpStatus { status, .. } => {
                write!(formatter, "agent_host returned HTTP {status}")
            }
            Self::Json { source } => write!(formatter, "agent_host JSON error: {source}"),
            Self::Header { header, source } => write!(
                formatter,
                "agent_host response header {header:?} was not valid text: {source}"
            ),
            Self::Utf8 { source } => write!(formatter, "agent_host stream was not UTF-8: {source}"),
            Self::UnexpectedJson { expected, .. } => {
                write!(
                    formatter,
                    "agent_host returned unexpected JSON; expected {expected}"
                )
            }
            Self::MissingField { field } => {
                write!(formatter, "agent_host response is missing field {field:?}")
            }
            Self::MissingHeader { header } => {
                write!(
                    formatter,
                    "agent_host response is missing header {header:?}"
                )
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
            Self::WebSocket { source } => Some(source),
            Self::Json { source } => Some(source),
            Self::Header { source, .. } => Some(source),
            Self::Utf8 { source } => Some(source),
            Self::InvalidBaseUrl { .. }
            | Self::InvalidTimeout { .. }
            | Self::InvalidWebSocketScheme { .. }
            | Self::InitialResponseTimeout { .. }
            | Self::WebSocketTimeout { .. }
            | Self::UrlCannotBeBase { .. }
            | Self::HttpStatus { .. }
            | Self::UnexpectedJson { .. }
            | Self::MissingHeader { .. }
            | Self::MissingField { .. }
            | Self::InvalidField { .. } => None,
        }
    }
}

pub struct AgentHostEventStream {
    response: reqwest::Response,
    pending: Vec<u8>,
    finished: bool,
    trace: AgentHostStreamTrace,
}

impl AgentHostEventStream {
    fn new(response: reqwest::Response, context: RequestTraceContext, status: StatusCode) -> Self {
        Self {
            response,
            pending: Vec::new(),
            finished: false,
            trace: AgentHostStreamTrace::new(context, status),
        }
    }

    pub async fn next_event(&mut self) -> AgentHostResult<Option<KernelEvent>> {
        loop {
            if let Some(line) = self.next_buffered_line() {
                match parse_stream_line(&line) {
                    Ok(Some(event)) => {
                        self.trace.record_event(&event);
                        return Ok(Some(event));
                    }
                    Ok(None) => {
                        self.trace.record_ignored_line();
                    }
                    Err(error) => {
                        self.trace.record_parse_error(&error);
                        return Err(error);
                    }
                }
                continue;
            }

            if self.finished {
                if self.pending.is_empty() {
                    self.trace.finish();
                    return Ok(None);
                }
                let line = std::mem::take(&mut self.pending);
                match parse_stream_line(&line) {
                    Ok(Some(event)) => {
                        self.trace.record_event(&event);
                        return Ok(Some(event));
                    }
                    Ok(None) => {
                        self.trace.record_ignored_line();
                    }
                    Err(error) => {
                        self.trace.record_parse_error(&error);
                        return Err(error);
                    }
                }

                self.trace.finish();
                return Ok(None);
            }

            let chunk = match self.response.chunk().await {
                Ok(chunk) => chunk,
                Err(source) => {
                    self.trace.record_read_error(&source);
                    return Err(AgentHostError::Http { source });
                }
            };
            match chunk {
                Some(chunk) => {
                    self.trace.record_chunk(chunk.len());
                    self.pending.extend_from_slice(&chunk);
                }
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

impl Drop for AgentHostEventStream {
    fn drop(&mut self) {
        if self.trace.ended {
            return;
        }
        if self.finished && self.pending.is_empty() {
            self.trace.finish();
        } else {
            self.trace.record_drop(self.pending.len());
        }
    }
}

#[derive(Debug)]
struct AgentHostStreamTrace {
    context: RequestTraceContext,
    status: StatusCode,
    started_at: Instant,
    chunks: usize,
    bytes: usize,
    events: usize,
    ignored_lines: usize,
    parse_errors: usize,
    read_errors: usize,
    ended: bool,
}

impl AgentHostStreamTrace {
    fn new(context: RequestTraceContext, status: StatusCode) -> Self {
        let trace = Self {
            context,
            status,
            started_at: Instant::now(),
            chunks: 0,
            bytes: 0,
            events: 0,
            ignored_lines: 0,
            parse_errors: 0,
            read_errors: 0,
            ended: false,
        };
        trace.log_start();
        trace
    }

    fn record_chunk(&mut self, byte_count: usize) {
        self.chunks = self.chunks.saturating_add(1);
        self.bytes = self.bytes.saturating_add(byte_count);
        tracing::debug!(
            target = %self.context.target,
            session_id = self.context.session_id(),
            gateway_id = self.context.gateway_id(),
            skill_id = self.context.skill_id(),
            status = self.status.as_u16(),
            chunk_bytes = byte_count,
            chunks = self.chunks,
            bytes = self.bytes,
            "agent_host stream chunk received"
        );
    }

    fn record_event(&mut self, event: &KernelEvent) {
        self.events = self.events.saturating_add(1);
        tracing::debug!(
            target = %self.context.target,
            session_id = self.context.session_id(),
            gateway_id = self.context.gateway_id(),
            skill_id = self.context.skill_id(),
            status = self.status.as_u16(),
            event_kind = stream_event_kind(event),
            events = self.events,
            chunks = self.chunks,
            bytes = self.bytes,
            "agent_host stream event parsed"
        );
    }

    fn record_ignored_line(&mut self) {
        self.ignored_lines = self.ignored_lines.saturating_add(1);
        tracing::trace!(
            target = %self.context.target,
            session_id = self.context.session_id(),
            gateway_id = self.context.gateway_id(),
            skill_id = self.context.skill_id(),
            status = self.status.as_u16(),
            ignored_lines = self.ignored_lines,
            events = self.events,
            "agent_host stream line ignored"
        );
    }

    fn record_parse_error(&mut self, error: &AgentHostError) {
        self.parse_errors = self.parse_errors.saturating_add(1);
        tracing::warn!(
            target = %self.context.target,
            session_id = self.context.session_id(),
            gateway_id = self.context.gateway_id(),
            skill_id = self.context.skill_id(),
            status = self.status.as_u16(),
            elapsed_ms = duration_ms(self.started_at.elapsed()),
            chunks = self.chunks,
            bytes = self.bytes,
            events = self.events,
            ignored_lines = self.ignored_lines,
            parse_errors = self.parse_errors,
            read_errors = self.read_errors,
            error_kind = agent_host_error_kind(error),
            error_status = agent_host_error_status(error),
            "agent_host stream parse failed"
        );
    }

    fn record_read_error(&mut self, source: &reqwest::Error) {
        self.read_errors = self.read_errors.saturating_add(1);
        tracing::warn!(
            target = %self.context.target,
            session_id = self.context.session_id(),
            gateway_id = self.context.gateway_id(),
            skill_id = self.context.skill_id(),
            status = self.status.as_u16(),
            elapsed_ms = duration_ms(self.started_at.elapsed()),
            chunks = self.chunks,
            bytes = self.bytes,
            events = self.events,
            ignored_lines = self.ignored_lines,
            parse_errors = self.parse_errors,
            read_errors = self.read_errors,
            error_kind = reqwest_error_kind(source),
            error_status = reqwest_error_status(source),
            "agent_host stream read failed"
        );
    }

    fn finish(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        tracing::info!(
            target = %self.context.target,
            session_id = self.context.session_id(),
            gateway_id = self.context.gateway_id(),
            skill_id = self.context.skill_id(),
            status = self.status.as_u16(),
            elapsed_ms = duration_ms(self.started_at.elapsed()),
            chunks = self.chunks,
            bytes = self.bytes,
            events = self.events,
            ignored_lines = self.ignored_lines,
            parse_errors = self.parse_errors,
            read_errors = self.read_errors,
            "agent_host stream ended"
        );
    }

    fn record_drop(&mut self, pending_bytes: usize) {
        self.ended = true;
        tracing::warn!(
            target = %self.context.target,
            session_id = self.context.session_id(),
            gateway_id = self.context.gateway_id(),
            skill_id = self.context.skill_id(),
            status = self.status.as_u16(),
            elapsed_ms = duration_ms(self.started_at.elapsed()),
            chunks = self.chunks,
            bytes = self.bytes,
            events = self.events,
            ignored_lines = self.ignored_lines,
            parse_errors = self.parse_errors,
            read_errors = self.read_errors,
            pending_bytes = pending_bytes,
            "agent_host stream dropped"
        );
    }

    fn log_start(&self) {
        tracing::info!(
            target = %self.context.target,
            session_id = self.context.session_id(),
            gateway_id = self.context.gateway_id(),
            skill_id = self.context.skill_id(),
            status = self.status.as_u16(),
            "agent_host stream started"
        );
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

fn stream_event_kind(event: &KernelEvent) -> &'static str {
    match event.get("type").and_then(Value::as_str) {
        Some("session/update") => match event
            .get("update")
            .and_then(Value::as_object)
            .and_then(|update| update.get("sessionUpdate"))
            .and_then(Value::as_str)
        {
            Some("agent_message_chunk") => "session/update.agent_message_chunk",
            Some("tool_call") => "session/update.tool_call",
            Some("tool_call_update") => "session/update.tool_call_update",
            Some("error") => "session/update.error",
            Some(_other) => "session/update.other",
            None => "session/update",
        },
        Some("reasoning_delta") => "reasoning_delta",
        Some("text_delta") => "text_delta",
        Some("tool_call") => "tool_call",
        Some("tool_result") => "tool_result",
        Some("start") => "start",
        Some("content") => "content",
        Some("error") => "error",
        Some("done") => "done",
        Some(_other) => "other",
        None => "unknown",
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
        convert::Infallible,
        error::Error,
        net::SocketAddr,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{
        Json, Router,
        body::{Body, Bytes},
        extract::{Path, Query, State},
        http::{
            HeaderMap, Method, StatusCode,
            header::{CONTENT_DISPOSITION, CONTENT_TYPE},
        },
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
    use tokio_stream::wrappers::ReceiverStream;

    use super::{AgentHostClient, AgentHostError, AgentHostSessionCreate, JsonObject};

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

    #[test]
    fn http_status_error_display_does_not_include_body() {
        let error = AgentHostError::HttpStatus {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: "secret token and stack trace".to_owned(),
        };

        let message = error.to_string();

        assert_eq!(
            message,
            "agent_host returned HTTP 500 Internal Server Error"
        );
        assert!(!message.contains("secret token"));
        assert!(!message.contains("stack trace"));
    }

    #[test]
    fn unexpected_json_error_display_does_not_include_value() {
        let error = AgentHostError::UnexpectedJson {
            expected: "object",
            value: json!({ "api_key": "secret-token", "stack": ["internal"] }),
        };

        let message = error.to_string();

        assert_eq!(
            message,
            "agent_host returned unexpected JSON; expected object"
        );
        assert!(!message.contains("secret-token"));
        assert!(!message.contains("internal"));
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
            .route("/skills/{skill_id}/download", get(download_skill))
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
        if session_id == "missing-session" {
            return Ok(StatusCode::NOT_FOUND);
        }
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
        if session_id == "never-starts" {
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Ok((
                StatusCode::OK,
                Body::from("{\"type\":\"content\",\"content\":\"late\"}\n"),
            )
                .into_response());
        }
        if session_id == "slow-session" {
            let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(2);
            tokio::spawn(async move {
                let _ignored = sender
                    .send(Ok(Bytes::from_static(
                        b"{\"type\":\"content\",\"content\":\"started\"}\n",
                    )))
                    .await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ignored = sender
                    .send(Ok(Bytes::from_static(
                        b"{\"type\":\"content\",\"content\":\"delayed\"}\n",
                    )))
                    .await;
            });
            return Ok((
                StatusCode::OK,
                Body::from_stream(ReceiverStream::new(receiver)),
            )
                .into_response());
        }
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

    async fn download_skill(
        State(state): State<TestState>,
        Path(skill_id): Path<String>,
    ) -> Result<Response, StatusCode> {
        state.record(
            Method::GET,
            format!("/skills/{skill_id}/download"),
            None,
            None,
        )?;
        Ok((
            [
                (CONTENT_TYPE, "text/markdown; charset=utf-8"),
                (CONTENT_DISPOSITION, "attachment; filename=\"SKILL.md\""),
            ],
            Body::from("# Skill"),
        )
            .into_response())
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
        let additional_paths = vec!["/workspace/extra".to_owned()];

        let response = client
            .create_session(AgentHostSessionCreate {
                session_id: "stable-session",
                telemetry_volume_identity: Some("telemetry-stable"),
                interaction_mode: "chat",
                harness: "copilot",
                skills: Some(&skills),
                env: Some(&env),
                additional_paths: Some(&additional_paths),
                workspace_mounts: None,
            })
            .await?;

        assert_eq!(response["session_id"], "session-1");
        assert_eq!(
            server.recorded()?,
            vec![RecordedRequest {
                method: Method::POST,
                path: "/sessions".to_owned(),
                query: None,
                body: Some(json!({
                    "session_id": "stable-session",
                    "telemetry_volume_identity": "telemetry-stable",
                    "interaction_mode": "chat",
                    "harness": "copilot",
                    "skills": ["skill-a", "skill-b"],
                    "env": { "KEY": "value" },
                    "additional_paths": ["/workspace/extra"]
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

        client
            .create_session(AgentHostSessionCreate {
                session_id: "stable-session",
                telemetry_volume_identity: None,
                interaction_mode: "chat",
                harness: "copilot",
                skills: None,
                env: Some(&env),
                additional_paths: None,
                workspace_mounts: None,
            })
            .await?;

        assert_eq!(
            server.recorded()?,
            vec![RecordedRequest {
                method: Method::POST,
                path: "/sessions".to_owned(),
                query: None,
                body: Some(json!({
                    "session_id": "stable-session",
                    "interaction_mode": "chat",
                    "harness": "copilot"
                })),
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
    async fn stream_message_times_out_before_initial_response()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let server = TestServer::start().await?;
        let client = AgentHostClient::new(&server.base_url, Duration::from_millis(1))?;

        let error = client.send_message("never-starts", "wait for it").await;

        assert!(matches!(
            error,
            Err(AgentHostError::InitialResponseTimeout { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn stream_message_is_not_limited_by_client_timeout()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        let server = TestServer::start().await?;
        let client = AgentHostClient::new(&server.base_url, Duration::from_millis(10))?;

        let events = client.send_message("slow-session", "wait for it").await?;

        assert_eq!(
            events,
            vec![
                object(json!({ "type": "content", "content": "started" }))?,
                object(json!({ "type": "content", "content": "delayed" }))?,
            ]
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
        client.destroy_session("missing-session").await?;
        client.create_skill("skill-1", &files).await?;
        client.get_skill("skill-1").await?;
        let download = client.download_skill("skill-1").await?;
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

        assert_eq!(download.content_type, "text/markdown; charset=utf-8");
        assert_eq!(
            download.content_disposition,
            "attachment; filename=\"SKILL.md\""
        );
        assert_eq!(download.body.as_slice(), b"# Skill");

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
                    method: Method::DELETE,
                    path: "/sessions/missing-session".to_owned(),
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
                    path: "/skills/skill-1/download".to_owned(),
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
