use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::Arc,
    time::Duration,
};

use axum::http::{HeaderValue, Method};
use reqwest::{Response, Url, header};

pub use memory_rs::{
    run_stream::RUN_CONTENT_TYPE as MEMORY_RUN_CONTENT_TYPE,
    wire::{
        ContentQuery, ErrorBody, ErrorEnvelope, JSON_CONTENT_TYPE as MEMORY_JSON_CONTENT_TYPE,
        LinksQuery, ListPagesQuery, MovePageWire, PageWire, RunRequestWire, WritePageWire,
    },
};

// The memory service caps runs at two minutes and whole requests at three.
const MEMORY_RUN_PROXY_TIMEOUT: Duration = Duration::from_secs(190);

#[derive(Clone, Debug)]
pub struct MemoryProxyClient {
    client: reqwest::Client,
    base_url: Result<Url, Arc<MemoryBaseUrlError>>,
    timeout: Duration,
}

impl MemoryProxyClient {
    #[must_use]
    pub fn new(base_url: &str, timeout: Duration) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: Url::parse(base_url).map_err(|source| {
                Arc::new(MemoryBaseUrlError {
                    raw: base_url.to_owned(),
                    message: source.to_string(),
                })
            }),
            timeout,
        }
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        query: Option<&str>,
        content_type: Option<&HeaderValue>,
        body: Vec<u8>,
        streaming: bool,
    ) -> Result<Response, MemoryProxyError> {
        let mut url = self.endpoint(path)?;
        url.set_query(query);
        let mut request = self
            .client
            .request(method, url)
            .timeout(self.request_timeout(streaming))
            .body(body);
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }

        request.send().await.map_err(MemoryProxyError::from_reqwest)
    }

    fn request_timeout(&self, streaming: bool) -> Duration {
        if streaming {
            self.timeout.max(MEMORY_RUN_PROXY_TIMEOUT)
        } else {
            self.timeout
        }
    }

    fn endpoint(&self, path: &str) -> Result<Url, MemoryProxyError> {
        let mut url = self
            .base_url
            .as_ref()
            .map_err(|error| MemoryProxyError::InvalidBaseUrl {
                raw: error.raw.clone(),
                message: error.message.clone(),
            })?
            .clone();
        let base_url = url.to_string();
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| MemoryProxyError::UrlCannotBeBase { base_url })?;
        segments.clear();
        segments.extend(path.trim_start_matches('/').split('/'));
        drop(segments);
        Ok(url)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MemoryBaseUrlError {
    raw: String,
    message: String,
}

#[derive(Debug)]
pub enum MemoryProxyError {
    InvalidBaseUrl { raw: String, message: String },
    UrlCannotBeBase { base_url: String },
    Timeout { message: String },
    Unavailable { message: String },
    Http { source: reqwest::Error },
    MalformedResponse { detail: String },
    ResponseTooLarge { limit: usize },
}

impl MemoryProxyError {
    fn from_reqwest(source: reqwest::Error) -> Self {
        if source.is_timeout() {
            return Self::Timeout {
                message: source.to_string(),
            };
        }
        if source.is_connect() {
            return Self::Unavailable {
                message: source.to_string(),
            };
        }
        Self::Http { source }
    }
}

impl Display for MemoryProxyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl { raw, message } => {
                write!(
                    formatter,
                    "invalid memory service base URL {raw:?}: {message}"
                )
            }
            Self::UrlCannotBeBase { base_url } => {
                write!(formatter, "memory service URL cannot be a base: {base_url}")
            }
            Self::Timeout { .. } => formatter.write_str("memory service request timed out"),
            Self::Unavailable { .. } => formatter.write_str("memory service is unavailable"),
            Self::Http { source } => write!(formatter, "memory service HTTP error: {source}"),
            Self::MalformedResponse { detail } => {
                write!(
                    formatter,
                    "memory service returned a malformed response: {detail}"
                )
            }
            Self::ResponseTooLarge { limit } => {
                write!(
                    formatter,
                    "memory service response exceeded the {limit}-byte limit"
                )
            }
        }
    }
}

impl Error for MemoryProxyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http { source } => Some(source),
            Self::InvalidBaseUrl { .. }
            | Self::UrlCannotBeBase { .. }
            | Self::Timeout { .. }
            | Self::Unavailable { .. }
            | Self::MalformedResponse { .. }
            | Self::ResponseTooLarge { .. } => None,
        }
    }
}
