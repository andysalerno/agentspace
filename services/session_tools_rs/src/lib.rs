use std::{fmt, time::Duration};

use clap::{Parser, Subcommand};
use reqwest::{StatusCode, Url, header};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const CLIENT_SERVICE_URL_ENV: &str = "AGENTSPACE_CLIENT_SERVICE_URL";
pub const SESSION_ID_ENV: &str = "AGENTSPACE_SESSION_ID";
pub const SESSION_CONTROL_TOKEN_ENV: &str = "AGENTSPACE_SESSION_CONTROL_TOKEN";
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const START_NEW_PATH: &str = "/internal/session-control/start-new";
const SUCCESS_MESSAGE: &str = "Fresh-session handoff accepted. The triggering message will be replayed; stop this response now and do not answer it.";

#[derive(Debug, Parser)]
#[command(
    name = "session-tools",
    version,
    about = "AgentSpace session lifecycle tools",
    propagate_version = true
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Replay the current message in a fresh `AgentSpace` session.
    StartNew,
}

#[derive(Clone, Debug)]
pub struct SessionControlConfig {
    service_url: Url,
    session_id: String,
    token: String,
}

impl SessionControlConfig {
    pub fn from_env() -> Result<Self, SessionToolsError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    pub fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, SessionToolsError> {
        let service_url_raw = required_env(&mut lookup, CLIENT_SERVICE_URL_ENV)?;
        let service_url = Url::parse(&service_url_raw).map_err(|_error| {
            SessionToolsError::configuration(format!(
                "{CLIENT_SERVICE_URL_ENV} must be a valid HTTP(S) URL"
            ))
        })?;
        if !matches!(service_url.scheme(), "http" | "https") {
            return Err(SessionToolsError::configuration(format!(
                "{CLIENT_SERVICE_URL_ENV} must use HTTP or HTTPS"
            )));
        }
        let session_id = required_env(&mut lookup, SESSION_ID_ENV)?;
        let token = required_env(&mut lookup, SESSION_CONTROL_TOKEN_ENV)?;
        Ok(Self {
            service_url,
            session_id,
            token,
        })
    }

    #[cfg(test)]
    fn new(service_url: &str, session_id: &str, token: &str) -> Result<Self, SessionToolsError> {
        Self::from_lookup(|name| match name {
            CLIENT_SERVICE_URL_ENV => Some(service_url.to_owned()),
            SESSION_ID_ENV => Some(session_id.to_owned()),
            SESSION_CONTROL_TOKEN_ENV => Some(token.to_owned()),
            _ => None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct StartNewResponse {
    accepted: bool,
    turn_id: String,
}

#[derive(Debug)]
pub struct StartNewResult {
    pub turn_id: String,
}

#[derive(Debug)]
pub struct CommandResult {
    pub exit_code: u8,
    pub stdout: String,
    pub stderr: String,
}

impl CommandResult {
    #[must_use]
    pub fn success(json_output: bool, result: &StartNewResult) -> Self {
        let stdout = if json_output {
            serde_json::to_string(&JsonSuccess {
                ok: true,
                action: "start-new",
                accepted: true,
                turn_id: &result.turn_id,
                message: SUCCESS_MESSAGE,
            })
            .unwrap_or_else(|_error| "{\"ok\":false}".to_owned())
        } else {
            SUCCESS_MESSAGE.to_owned()
        };
        Self {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        }
    }

    #[must_use]
    pub fn failure(json_output: bool, error: &SessionToolsError) -> Self {
        let message = error.to_string();
        let stderr = if json_output {
            serde_json::to_string(&JsonFailure {
                ok: false,
                error: JsonError {
                    kind: error.kind(),
                    message: &message,
                },
            })
            .unwrap_or_else(|_error| "{\"ok\":false}".to_owned())
        } else {
            format!("session-tools: {message}")
        };
        Self {
            exit_code: error.exit_code(),
            stdout: String::new(),
            stderr,
        }
    }
}

#[derive(Serialize)]
struct JsonSuccess<'a> {
    ok: bool,
    action: &'static str,
    accepted: bool,
    turn_id: &'a str,
    message: &'static str,
}

#[derive(Serialize)]
struct JsonFailure<'a> {
    ok: bool,
    error: JsonError<'a>,
}

#[derive(Serialize)]
struct JsonError<'a> {
    kind: &'static str,
    message: &'a str,
}

#[derive(Debug)]
pub enum SessionToolsError {
    Configuration(String),
    Transport(String),
    Server(StatusCode),
    Protocol(String),
}

impl SessionToolsError {
    const fn configuration(message: String) -> Self {
        Self::Configuration(message)
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Configuration(_) => "configuration",
            Self::Transport(_) => "transport",
            Self::Server(_) => "server",
            Self::Protocol(_) => "protocol",
        }
    }

    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Configuration(_) => 2,
            Self::Transport(_) | Self::Server(_) | Self::Protocol(_) => 1,
        }
    }
}

impl fmt::Display for SessionToolsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message) | Self::Transport(message) | Self::Protocol(message) => {
                formatter.write_str(message)
            }
            Self::Server(status) => {
                write!(
                    formatter,
                    "session handoff request failed with HTTP {status}"
                )
            }
        }
    }
}

impl std::error::Error for SessionToolsError {}

pub async fn run(cli: &Cli) -> CommandResult {
    let config = match SessionControlConfig::from_env() {
        Ok(config) => config,
        Err(error) => return CommandResult::failure(cli.json, &error),
    };
    let result = match cli.command {
        Command::StartNew => start_new(&config, DEFAULT_REQUEST_TIMEOUT).await,
    };
    match result {
        Ok(result) => CommandResult::success(cli.json, &result),
        Err(error) => CommandResult::failure(cli.json, &error),
    }
}

pub async fn start_new(
    config: &SessionControlConfig,
    timeout: Duration,
) -> Result<StartNewResult, SessionToolsError> {
    let endpoint = config.service_url.join(START_NEW_PATH).map_err(|_error| {
        SessionToolsError::configuration(format!(
            "{CLIENT_SERVICE_URL_ENV} cannot be used as a service base URL"
        ))
    })?;
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|_error| {
            SessionToolsError::Transport("failed to initialize HTTP client".to_owned())
        })?;
    let response = client
        .post(endpoint)
        .header(header::AUTHORIZATION, format!("Bearer {}", config.token))
        .json(&json!({ "session_id": config.session_id }))
        .send()
        .await
        .map_err(|error| map_transport_error(&error))?;
    if response.status() != StatusCode::ACCEPTED {
        return Err(SessionToolsError::Server(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|size| usize::try_from(size).map_or(true, |size| size > MAX_RESPONSE_BYTES))
    {
        return Err(SessionToolsError::Protocol(
            "session handoff response was too large".to_owned(),
        ));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| map_transport_error(&error))?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(SessionToolsError::Protocol(
            "session handoff response was too large".to_owned(),
        ));
    }
    let response = serde_json::from_slice::<StartNewResponse>(&body).map_err(|_error| {
        SessionToolsError::Protocol("session handoff returned an invalid response".to_owned())
    })?;
    if !response.accepted || response.turn_id.is_empty() {
        return Err(SessionToolsError::Protocol(
            "session handoff was not accepted".to_owned(),
        ));
    }
    Ok(StartNewResult {
        turn_id: response.turn_id,
    })
}

fn required_env(
    lookup: &mut impl FnMut(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, SessionToolsError> {
    lookup(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| SessionToolsError::configuration(format!("{name} is required")))
}

fn map_transport_error(error: &reqwest::Error) -> SessionToolsError {
    if error.is_timeout() {
        SessionToolsError::Transport("session handoff request timed out".to_owned())
    } else if error.is_connect() {
        SessionToolsError::Transport("could not connect to client_service".to_owned())
    } else {
        SessionToolsError::Transport("session handoff transport failed".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::post,
    };
    use clap::Parser as _;
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, task::JoinHandle, time::sleep};

    use super::{
        CLIENT_SERVICE_URL_ENV, Cli, Command, CommandResult, SESSION_CONTROL_TOKEN_ENV,
        SESSION_ID_ENV, SessionControlConfig, SessionToolsError, start_new,
    };

    #[derive(Clone)]
    struct MockState {
        response: MockResponse,
        authorization: Arc<Mutex<Option<String>>>,
    }

    #[derive(Clone, Copy)]
    enum MockResponse {
        Accepted,
        Unauthorized,
        Conflict,
        Malformed,
        Delayed,
    }

    struct MockServer {
        base_url: String,
        authorization: Arc<Mutex<Option<String>>>,
        handle: JoinHandle<Result<(), std::io::Error>>,
    }

    impl MockServer {
        async fn start(response: MockResponse) -> Result<Self, Box<dyn Error + Send + Sync>> {
            let authorization = Arc::new(Mutex::new(None));
            let state = MockState {
                response,
                authorization: authorization.clone(),
            };
            let app = Router::new()
                .route("/internal/session-control/start-new", post(mock_start_new))
                .with_state(state);
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let handle = tokio::spawn(axum::serve(listener, app).into_future());
            Ok(Self {
                base_url: format!("http://{address}"),
                authorization,
                handle,
            })
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn mock_start_new(
        State(state): State<MockState>,
        headers: HeaderMap,
        Json(_body): Json<Value>,
    ) -> Response {
        if let Ok(mut authorization) = state.authorization.lock() {
            *authorization = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
        }
        match state.response {
            MockResponse::Accepted => (
                StatusCode::ACCEPTED,
                Json(json!({ "accepted": true, "turn_id": "turn-one" })),
            )
                .into_response(),
            MockResponse::Unauthorized => StatusCode::UNAUTHORIZED.into_response(),
            MockResponse::Conflict => StatusCode::CONFLICT.into_response(),
            MockResponse::Malformed => (StatusCode::ACCEPTED, "not-json").into_response(),
            MockResponse::Delayed => {
                sleep(Duration::from_millis(100)).await;
                (
                    StatusCode::ACCEPTED,
                    Json(json!({ "accepted": true, "turn_id": "turn-one" })),
                )
                    .into_response()
            }
        }
    }

    #[test]
    fn parses_the_deliberately_small_cli_surface() -> Result<(), Box<dyn Error + Send + Sync>> {
        let cli = Cli::try_parse_from(["session-tools", "start-new"])?;
        assert!(matches!(cli.command, Command::StartNew));
        assert!(!cli.json);
        let cli = Cli::try_parse_from(["session-tools", "start-new", "--json"])?;
        assert!(cli.json);
        assert!(Cli::try_parse_from(["session-tools"]).is_err());
        assert!(Cli::try_parse_from(["session-tools", "unknown"]).is_err());
        Ok(())
    }

    #[test]
    fn validates_environment_without_echoing_values() -> Result<(), Box<dyn Error + Send + Sync>> {
        for missing in [
            CLIENT_SERVICE_URL_ENV,
            SESSION_ID_ENV,
            SESSION_CONTROL_TOKEN_ENV,
        ] {
            let error = SessionControlConfig::from_lookup(|name| {
                (name != missing).then(|| match name {
                    CLIENT_SERVICE_URL_ENV => "http://client-service:8002".to_owned(),
                    SESSION_ID_ENV => "session-one".to_owned(),
                    SESSION_CONTROL_TOKEN_ENV => "super-secret-token".to_owned(),
                    _ => String::new(),
                })
            })
            .err()
            .ok_or("missing environment unexpectedly succeeded")?;
            assert_eq!(error.exit_code(), 2);
            assert!(!error.to_string().contains("super-secret-token"));
        }
        let error = SessionControlConfig::from_lookup(|name| {
            Some(match name {
                CLIENT_SERVICE_URL_ENV => "not a url".to_owned(),
                SESSION_ID_ENV => "session-one".to_owned(),
                SESSION_CONTROL_TOKEN_ENV => "super-secret-token".to_owned(),
                _ => String::new(),
            })
        })
        .err()
        .ok_or("invalid URL unexpectedly succeeded")?;
        assert_eq!(error.exit_code(), 2);
        assert!(!error.to_string().contains("super-secret-token"));
        Ok(())
    }

    #[tokio::test]
    async fn accepts_handoff_and_supports_stable_output() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let server = MockServer::start(MockResponse::Accepted).await?;
        let config =
            SessionControlConfig::new(&server.base_url, "session-one", "super-secret-token")?;
        let result = start_new(&config, Duration::from_secs(1)).await?;
        assert_eq!(result.turn_id, "turn-one");
        assert_eq!(
            server
                .authorization
                .lock()
                .map_err(|_error| "authorization lock poisoned")?
                .as_deref(),
            Some("Bearer super-secret-token")
        );

        let human = CommandResult::success(false, &result);
        assert_eq!(human.exit_code, 0);
        assert!(human.stdout.contains("stop this response now"));
        let machine = CommandResult::success(true, &result);
        assert_eq!(serde_json::from_str::<Value>(&machine.stdout)?["ok"], true);
        assert!(!human.stdout.contains("super-secret-token"));
        assert!(!machine.stdout.contains("super-secret-token"));
        Ok(())
    }

    #[tokio::test]
    async fn maps_server_protocol_timeout_and_connection_failures()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        for (response, expected_status) in [
            (MockResponse::Unauthorized, StatusCode::UNAUTHORIZED),
            (MockResponse::Conflict, StatusCode::CONFLICT),
        ] {
            let server = MockServer::start(response).await?;
            let config = SessionControlConfig::new(&server.base_url, "session-one", "secret")?;
            let error = start_new(&config, Duration::from_secs(1))
                .await
                .err()
                .ok_or("server error unexpectedly succeeded")?;
            assert!(
                matches!(error, SessionToolsError::Server(status) if status == expected_status)
            );
        }

        let server = MockServer::start(MockResponse::Malformed).await?;
        let config = SessionControlConfig::new(&server.base_url, "session-one", "secret")?;
        let error = start_new(&config, Duration::from_secs(1))
            .await
            .err()
            .ok_or("malformed response unexpectedly succeeded")?;
        assert_eq!(error.kind(), "protocol");

        let server = MockServer::start(MockResponse::Delayed).await?;
        let config = SessionControlConfig::new(&server.base_url, "session-one", "secret")?;
        let error = start_new(&config, Duration::from_millis(10))
            .await
            .err()
            .ok_or("timeout unexpectedly succeeded")?;
        assert_eq!(error.kind(), "transport");
        assert!(error.to_string().contains("timed out"));

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        drop(listener);
        let config =
            SessionControlConfig::new(&format!("http://{address}"), "session-one", "secret")?;
        let error = start_new(&config, Duration::from_millis(100))
            .await
            .err()
            .ok_or("connection unexpectedly succeeded")?;
        assert_eq!(error.kind(), "transport");
        assert!(!error.to_string().contains("secret"));
        Ok(())
    }

    #[test]
    fn failure_output_never_contains_capability() {
        let error = SessionToolsError::Transport("request failed".to_owned());
        for json_output in [false, true] {
            let output = CommandResult::failure(json_output, &error);
            assert_eq!(output.exit_code, 1);
            assert!(!output.stderr.contains("super-secret-token"));
        }
    }
}
