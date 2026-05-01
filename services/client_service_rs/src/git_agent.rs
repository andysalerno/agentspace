use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    str::Utf8Error,
    sync::Arc,
    time::Duration,
};

use reqwest::{Method, StatusCode, Url};
use serde_json::{Map, Value};

pub type JsonObject = Map<String, Value>;
pub type JsonArray = Vec<JsonObject>;
pub type GitAgentResult<T> = Result<T, GitAgentError>;

#[derive(Clone, Debug)]
pub struct GitAgentClient {
    client: reqwest::Client,
    base_url: Result<Url, Arc<GitAgentBaseUrlError>>,
    timeout: Duration,
}

impl GitAgentClient {
    #[must_use]
    pub fn new(base_url: &str, timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: Url::parse(base_url).map_err(|source| {
                Arc::new(GitAgentBaseUrlError {
                    raw: base_url.to_owned(),
                    message: source.to_string(),
                })
            }),
            timeout,
        }
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn base_url(&self) -> GitAgentResult<&Url> {
        self.base_url
            .as_ref()
            .map_err(|error| GitAgentError::InvalidBaseUrl {
                raw: error.raw.clone(),
                message: error.message.clone(),
            })
    }

    pub async fn status(&self) -> GitAgentResult<JsonObject> {
        self.request_object(Method::GET, self.endpoint(&["status"])?)
            .await
    }

    pub async fn list_requests(&self) -> GitAgentResult<Value> {
        self.request_json(Method::GET, self.endpoint(&["patch-requests"])?)
            .await
    }

    pub async fn get_request(&self, request_id: &str) -> GitAgentResult<JsonObject> {
        self.request_object(Method::GET, self.endpoint(&["patch-requests", request_id])?)
            .await
    }

    pub async fn rerun_review(&self, request_id: &str) -> GitAgentResult<JsonObject> {
        self.request_object(
            Method::POST,
            self.endpoint(&["patch-requests", request_id, "rerun-review"])?,
        )
        .await
    }

    fn endpoint(&self, segments: &[&str]) -> GitAgentResult<Url> {
        let mut url = self.base_url()?.clone();
        {
            let base_url = url.to_string();
            let mut path_segments = url
                .path_segments_mut()
                .map_err(|()| GitAgentError::UrlCannotBeBase { base_url })?;
            path_segments.clear();
            path_segments.extend(segments);
        }
        Ok(url)
    }

    async fn request_object(&self, method: Method, url: Url) -> GitAgentResult<JsonObject> {
        match self.request_json(method, url).await? {
            Value::Object(object) => Ok(object),
            other => Err(GitAgentError::UnexpectedJson {
                expected: "object",
                value: other,
            }),
        }
    }

    async fn request_json(&self, method: Method, url: Url) -> GitAgentResult<Value> {
        let response = self
            .client
            .request(method, url)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|source| GitAgentError::Http { source })?;
        let response = ensure_success(response).await?;
        response
            .json::<Value>()
            .await
            .map_err(|source| GitAgentError::Http { source })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitAgentBaseUrlError {
    raw: String,
    message: String,
}

#[derive(Debug)]
pub enum GitAgentError {
    InvalidBaseUrl {
        raw: String,
        message: String,
    },
    UrlCannotBeBase {
        base_url: String,
    },
    Http {
        source: reqwest::Error,
    },
    HttpStatus {
        status: StatusCode,
        body: String,
    },
    Utf8 {
        source: Utf8Error,
    },
    UnexpectedJson {
        expected: &'static str,
        value: Value,
    },
}

impl Display for GitAgentError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl { raw, message } => {
                write!(formatter, "invalid git_agent base URL {raw:?}: {message}")
            }
            Self::UrlCannotBeBase { base_url } => {
                write!(formatter, "git_agent base URL cannot be a base: {base_url}")
            }
            Self::Http { source } => write!(formatter, "git_agent HTTP error: {source}"),
            Self::HttpStatus { status, .. } => {
                write!(formatter, "git_agent returned HTTP {status}")
            }
            Self::Utf8 { source } => {
                write!(formatter, "git_agent response was not UTF-8: {source}")
            }
            Self::UnexpectedJson { expected, .. } => {
                write!(
                    formatter,
                    "git_agent returned unexpected JSON; expected {expected}"
                )
            }
        }
    }
}

impl Error for GitAgentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http { source } => Some(source),
            Self::Utf8 { source } => Some(source),
            Self::InvalidBaseUrl { .. }
            | Self::UrlCannotBeBase { .. }
            | Self::HttpStatus { .. }
            | Self::UnexpectedJson { .. } => None,
        }
    }
}

async fn ensure_success(response: reqwest::Response) -> GitAgentResult<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    Err(http_status_error(response).await)
}

async fn http_status_error(response: reqwest::Response) -> GitAgentError {
    let status = response.status();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(source) => return GitAgentError::Http { source },
    };
    match std::str::from_utf8(&bytes) {
        Ok(body) => GitAgentError::HttpStatus {
            status,
            body: body.to_owned(),
        },
        Err(source) => GitAgentError::Utf8 { source },
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error, time::Duration};

    use axum::{Json, Router, http::StatusCode, response::IntoResponse, routing::get};
    use serde_json::json;
    use tokio::{net::TcpListener, task::JoinHandle};

    use super::{GitAgentClient, GitAgentError};

    struct TestServer {
        base_url: String,
        handle: JoinHandle<Result<(), std::io::Error>>,
    }

    impl TestServer {
        async fn start() -> Result<Self, Box<dyn Error + Send + Sync>> {
            let app = Router::new()
                .route("/status", get(status))
                .route("/patch-requests", get(list_requests))
                .route("/patch-requests/{request_id}", get(get_request));
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = listener.local_addr()?;
            let handle = tokio::spawn(axum::serve(listener, app).into_future());
            Ok(Self {
                base_url: format!("http://{address}"),
                handle,
            })
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn status() -> Json<serde_json::Value> {
        Json(json!({ "status": "ok" }))
    }

    async fn list_requests() -> Json<serde_json::Value> {
        Json(json!([{ "request_id": "request-one" }]))
    }

    async fn get_request() -> impl IntoResponse {
        Json(json!({ "request_id": "request-one", "raw_patch": "diff --git a/a b/a" }))
    }

    #[tokio::test]
    async fn client_fetches_status_and_requests() -> Result<(), Box<dyn Error + Send + Sync>> {
        let server = TestServer::start().await?;
        let client = GitAgentClient::new(&server.base_url, Duration::from_secs(5));

        assert_eq!(client.status().await?["status"], "ok");
        assert_eq!(
            client.list_requests().await?,
            json!([{ "request_id": "request-one" }])
        );
        assert_eq!(
            client.get_request("request-one").await?["raw_patch"],
            "diff --git a/a b/a"
        );
        Ok(())
    }

    #[tokio::test]
    async fn client_surfaces_upstream_status() -> Result<(), Box<dyn Error + Send + Sync>> {
        let app = Router::new().route("/status", get(|| async { StatusCode::BAD_GATEWAY }));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let handle = tokio::spawn(axum::serve(listener, app).into_future());
        let client = GitAgentClient::new(&format!("http://{address}"), Duration::from_secs(5));

        let error = client.status().await;
        handle.abort();
        assert!(matches!(
            error,
            Err(GitAgentError::HttpStatus { status, .. }) if status == StatusCode::BAD_GATEWAY
        ));
        Ok(())
    }
}
