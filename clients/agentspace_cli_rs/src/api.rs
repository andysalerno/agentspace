//! Reusable JSON-over-HTTP client for `AgentSpace` host APIs.

use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    time::Duration,
};

use reqwest::{Method, StatusCode, Url};
use serde::{Serialize, de::DeserializeOwned};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ApiClient {
    client: reqwest::Client,
    base_url: Url,
}

impl ApiClient {
    pub fn new(base_url: &str) -> Result<Self, ApiError> {
        let mut base_url = Url::parse(base_url).map_err(|source| ApiError::InvalidBaseUrl {
            raw: base_url.to_owned(),
            message: source.to_string(),
        })?;
        if base_url.cannot_be_a_base() {
            return Err(ApiError::UrlCannotBeBase {
                base_url: base_url.to_string(),
            });
        }
        base_url.set_query(None);
        base_url.set_fragment(None);
        Ok(Self {
            client: reqwest::Client::new(),
            base_url,
        })
    }

    pub async fn get<R: DeserializeOwned>(&self, path: &[&str]) -> Result<R, ApiError> {
        self.request_json::<(), R>(Method::GET, path, None).await
    }

    pub async fn post<P: Serialize + Sync + ?Sized, R: DeserializeOwned>(
        &self,
        path: &[&str],
        payload: Option<&P>,
    ) -> Result<R, ApiError> {
        self.request_json(Method::POST, path, payload).await
    }

    pub async fn put<P: Serialize + Sync + ?Sized, R: DeserializeOwned>(
        &self,
        path: &[&str],
        payload: &P,
    ) -> Result<R, ApiError> {
        self.request_json(Method::PUT, path, Some(payload)).await
    }

    pub async fn delete(&self, path: &[&str]) -> Result<(), ApiError> {
        let response = self.send(Method::DELETE, path, None::<&()>).await?;
        response_body(response).await.map(|_| ())
    }

    async fn request_json<P: Serialize + Sync + ?Sized, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &[&str],
        payload: Option<&P>,
    ) -> Result<R, ApiError> {
        let response = self.send(method, path, payload).await?;
        let body = response_body(response).await?;
        serde_json::from_slice(&body).map_err(|source| ApiError::MalformedResponse {
            message: source.to_string(),
        })
    }

    async fn send<P: Serialize + Sync + ?Sized>(
        &self,
        method: Method,
        path: &[&str],
        payload: Option<&P>,
    ) -> Result<reqwest::Response, ApiError> {
        let url = self.endpoint(path)?;
        let mut request = self.client.request(method, url).timeout(REQUEST_TIMEOUT);
        if let Some(payload) = payload {
            request = request.json(payload);
        }
        request.send().await.map_err(ApiError::from_reqwest)
    }

    fn endpoint(&self, path: &[&str]) -> Result<Url, ApiError> {
        let mut url = self.base_url.clone();
        let base_url = url.to_string();
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| ApiError::UrlCannotBeBase { base_url })?;
        segments.pop_if_empty();
        for segment in path {
            segments.push(segment);
        }
        drop(segments);
        Ok(url)
    }
}

async fn response_body(mut response: reqwest::Response) -> Result<Vec<u8>, ApiError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ApiError::ResponseTooLarge {
            limit: MAX_RESPONSE_BYTES,
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ApiError::from_reqwest)? {
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > MAX_RESPONSE_BYTES)
        {
            return Err(ApiError::ResponseTooLarge {
                limit: MAX_RESPONSE_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    if status.is_success() {
        return Ok(body);
    }
    let fallback = status
        .canonical_reason()
        .unwrap_or("request failed")
        .to_owned();
    let detail = serde_json::from_slice::<ErrorEnvelope>(&body).map_or_else(
        |_| {
            if body.is_empty() {
                fallback
            } else {
                String::from_utf8_lossy(&body).into_owned()
            }
        },
        |error| error.detail,
    );
    Err(ApiError::Response { status, detail })
}

#[derive(serde::Deserialize)]
struct ErrorEnvelope {
    detail: String,
}

#[derive(Debug)]
pub enum ApiError {
    InvalidBaseUrl { raw: String, message: String },
    UrlCannotBeBase { base_url: String },
    Timeout { message: String },
    Unavailable { message: String },
    Http { source: reqwest::Error },
    Response { status: StatusCode, detail: String },
    MalformedResponse { message: String },
    ResponseTooLarge { limit: usize },
}

impl ApiError {
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

impl Display for ApiError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl { raw, message } => {
                write!(formatter, "invalid API base URL {raw:?}: {message}")
            }
            Self::UrlCannotBeBase { base_url } => {
                write!(formatter, "API URL cannot be a base: {base_url}")
            }
            Self::Timeout { .. } => formatter.write_str("API request timed out"),
            Self::Unavailable { .. } => formatter.write_str("API is unavailable"),
            Self::Http { source } => write!(formatter, "API request failed: {source}"),
            Self::Response { status, detail } => {
                write!(formatter, "API returned {}: {detail}", status.as_u16())
            }
            Self::MalformedResponse { message } => {
                write!(formatter, "API returned malformed JSON: {message}")
            }
            Self::ResponseTooLarge { limit } => {
                write!(formatter, "API response exceeded the {limit}-byte limit")
            }
        }
    }
}

impl Error for ApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http { source } => Some(source),
            Self::InvalidBaseUrl { .. }
            | Self::UrlCannotBeBase { .. }
            | Self::Timeout { .. }
            | Self::Unavailable { .. }
            | Self::Response { .. }
            | Self::MalformedResponse { .. }
            | Self::ResponseTooLarge { .. } => None,
        }
    }
}
