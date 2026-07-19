//! [`HttpMemoryClient`]: a [`crate::client::MemoryClient`] implementation
//! backed by `reqwest`, selected when the CLI is configured with
//! `--uri`/`AGENTSPACE_MEMORY_URI`.
//!
//! Every method maps transport failures (connection refused, DNS failure,
//! a bounded timeout) to [`MemoryError::Unavailable`] and an unexpected or
//! invalid response shape to [`MemoryError::MalformedResponse`]; neither
//! ever falls back to local storage; the caller (`memory_rs::cli`) is
//! responsible for surfacing the resulting error rather than retrying
//! locally.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::{StatusCode, Url, header};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::AsyncWriteExt as _;
use tokio_stream::StreamExt as _;

use crate::{
    client::{CancelFuture, MemoryClient, OutputSink},
    command_runner::{RunLimits, RunOutcome},
    error::MemoryError,
    model::{
        CheckReport, LinksReport, ListFilter, MoveOutcome, MovePageRequest, Page, PageSummary,
        QueryRequest, RemovePageRequest, TagCount, WritePageRequest,
    },
    path::PagePath,
    run_stream::{FrameDecoder, RUN_CONTENT_TYPE, RunFrame},
    wire::{
        ErrorEnvelope, JSON_CONTENT_TYPE, MovePageWire, PageWire, RunRequestWire, WritePageWire,
    },
};

/// A backward-compatible alias for [`HttpMemoryClient`].
///
/// `RemoteMemoryClient` was the milestone-2 placeholder name; the type is
/// kept so any in-progress call site or documentation referring to it
/// keeps compiling.
pub type RemoteMemoryClient = HttpMemoryClient;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Extra time allowed on top of a `/v1/run` request's own timeout, so the
/// HTTP layer's request timeout never fires before the server's own
/// `RunLimits::timeout` has a chance to produce a terminal frame.
const RUN_STREAM_TIMEOUT_BUFFER: Duration = Duration::from_secs(10);
/// The largest JSON response body this client will buffer before giving up
/// and reporting [`MemoryError::TooLarge`], bounding memory use even
/// against a malformed or hostile server.
const MAX_JSON_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
struct InvalidBaseUrl {
    raw: String,
    message: String,
}

/// An HTTP [`MemoryClient`] over the `/v1/...` contract implemented by
/// [`crate::server`].
#[derive(Clone, Debug)]
pub struct HttpMemoryClient {
    client: reqwest::Client,
    base_url: Result<Url, Arc<InvalidBaseUrl>>,
}

impl HttpMemoryClient {
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        let raw = uri.into();
        let base_url = Url::parse(&raw).map_err(|source| {
            Arc::new(InvalidBaseUrl {
                raw: raw.clone(),
                message: source.to_string(),
            })
        });
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    fn endpoint(&self, path: &str) -> Result<Url, MemoryError> {
        let mut url = self
            .base_url
            .as_ref()
            .map_err(|error| {
                MemoryError::unavailable(format!(
                    "invalid memory service URI {:?}: {}",
                    error.raw, error.message
                ))
            })?
            .clone();
        url.set_query(None);
        {
            let cannot_be_base =
                MemoryError::unavailable(format!("memory service URI {url} cannot be a base"));
            let mut segments = url.path_segments_mut().map_err(|()| cannot_be_base)?;
            segments.clear();
            segments.extend(path.trim_start_matches('/').split('/'));
        }
        Ok(url)
    }

    async fn read_bounded(&self, mut response: reqwest::Response) -> Result<Vec<u8>, MemoryError> {
        let mut body = Vec::new();
        loop {
            let chunk = response
                .chunk()
                .await
                .map_err(|error| map_transport_error(&error))?;
            let Some(chunk) = chunk else { break };
            if body.len().saturating_add(chunk.len()) > MAX_JSON_RESPONSE_BYTES {
                return Err(MemoryError::too_large(
                    "memory service response",
                    MAX_JSON_RESPONSE_BYTES,
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    fn require_content_type(
        response: &reqwest::Response,
        expected: &'static str,
    ) -> Result<(), MemoryError> {
        let actual = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if actual == Some(expected) {
            return Ok(());
        }
        Err(MemoryError::malformed_response(format!(
            "expected content-type {expected:?}, got {actual:?}"
        )))
    }

    /// Sends a JSON request and returns the deserialized success body, or a
    /// [`MemoryError`] mapped from a non-success response or a malformed
    /// response shape.
    async fn json_request<Res: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&(impl Serialize + Sync)>,
    ) -> Result<Res, MemoryError> {
        let bytes = self.json_request_raw(method, path, query, body).await?;
        serde_json::from_slice(&bytes)
            .map_err(|error| MemoryError::malformed_response(error.to_string()))
    }

    /// Like [`Self::json_request`], but for endpoints that may reply
    /// `204 No Content` on success (no body to deserialize).
    async fn json_request_no_content(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<(), MemoryError> {
        let mut url = self.endpoint(path)?;
        url.query_pairs_mut().extend_pairs(query);
        let response = self
            .client
            .request(method, url)
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        let status = response.status();
        if status == StatusCode::NO_CONTENT {
            return Ok(());
        }
        if status.is_success() {
            Self::require_content_type(&response, JSON_CONTENT_TYPE)?;
            let _ = self.read_bounded(response).await?;
            return Ok(());
        }
        let bytes = self.read_bounded(response).await?;
        Err(parse_error_body(status, &bytes))
    }

    async fn json_request_raw(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&(impl Serialize + Sync)>,
    ) -> Result<Vec<u8>, MemoryError> {
        let mut url = self.endpoint(path)?;
        url.query_pairs_mut().extend_pairs(query);
        let mut request = self
            .client
            .request(method, url)
            .timeout(DEFAULT_REQUEST_TIMEOUT);
        if let Some(body) = body {
            let payload = serde_json::to_vec(body)
                .map_err(|error| MemoryError::internal(error.to_string()))?;
            request = request
                .header(header::CONTENT_TYPE, JSON_CONTENT_TYPE)
                .body(payload);
        }
        let response = request
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        let status = response.status();
        if !status.is_success() {
            let bytes = self.read_bounded(response).await?;
            return Err(parse_error_body(status, &bytes));
        }
        Self::require_content_type(&response, JSON_CONTENT_TYPE)?;
        self.read_bounded(response).await
    }
}

fn map_transport_error(source: &reqwest::Error) -> MemoryError {
    if source.is_timeout() {
        return MemoryError::unavailable(format!("request timed out: {source}"));
    }
    if source.is_connect() {
        return MemoryError::unavailable(format!("connection failed: {source}"));
    }
    MemoryError::unavailable(source.to_string())
}

fn parse_error_body(status: StatusCode, bytes: &[u8]) -> MemoryError {
    serde_json::from_slice::<ErrorEnvelope>(bytes).map_or_else(
        |error| {
            MemoryError::malformed_response(format!(
                "memory service returned status {status} with an unparsable error body: {error}"
            ))
        },
        ErrorEnvelope::into_memory_error,
    )
}

#[async_trait]
impl MemoryClient for HttpMemoryClient {
    async fn write_page(&self, request: WritePageRequest) -> Result<Page, MemoryError> {
        let wire = WritePageWire {
            title: request.title,
            tags: request.tags,
            body: request.body,
            overwrite: request.overwrite,
            expected_revision: request.expected_revision,
            actor: request.actor,
        };
        let page: PageWire = self
            .json_request(
                reqwest::Method::PUT,
                "/v1/pages/content",
                &[("path", request.path.as_str())],
                Some(&wire),
            )
            .await?;
        page.try_into()
    }

    async fn read_page(&self, path: PagePath) -> Result<Page, MemoryError> {
        let page: PageWire = self
            .json_request::<PageWire>(
                reqwest::Method::GET,
                "/v1/pages/content",
                &[("path", path.as_str())],
                None::<&()>,
            )
            .await?;
        page.try_into()
    }

    async fn move_page(&self, request: MovePageRequest) -> Result<MoveOutcome, MemoryError> {
        let wire = MovePageWire {
            source: request.source.as_str(),
            destination: request.destination.as_str(),
            expected_revision: request.expected_revision,
            actor: request.actor,
        };
        self.json_request(reqwest::Method::POST, "/v1/pages/move", &[], Some(&wire))
            .await
    }

    async fn remove_page(&self, request: RemovePageRequest) -> Result<(), MemoryError> {
        let mut query = vec![("path", request.path.as_str())];
        if let Some(expected) = request.expected_revision {
            query.push(("expected_revision", expected));
        }
        self.json_request_no_content(reqwest::Method::DELETE, "/v1/pages/content", &query)
            .await
    }

    async fn list_pages(&self, filter: ListFilter) -> Result<Vec<PageSummary>, MemoryError> {
        self.json_request(
            reqwest::Method::GET,
            "/v1/pages",
            &list_filter_query(&filter, None),
            None::<&()>,
        )
        .await
    }

    async fn query_pages(&self, request: QueryRequest) -> Result<Vec<PageSummary>, MemoryError> {
        self.json_request(
            reqwest::Method::GET,
            "/v1/pages",
            &list_filter_query(&request.filter, Some(request.text)),
            None::<&()>,
        )
        .await
    }

    async fn list_tags(&self) -> Result<Vec<TagCount>, MemoryError> {
        self.json_request(reqwest::Method::GET, "/v1/tags", &[], None::<&()>)
            .await
    }

    async fn links(
        &self,
        path: PagePath,
        include_backlinks: bool,
    ) -> Result<LinksReport, MemoryError> {
        let query = [
            ("path", path.as_str()),
            ("backlinks", include_backlinks.to_string()),
        ];
        self.json_request(reqwest::Method::GET, "/v1/links", &query, None::<&()>)
            .await
    }

    async fn check(&self) -> Result<CheckReport, MemoryError> {
        self.json_request(reqwest::Method::GET, "/v1/check", &[], None::<&()>)
            .await
    }

    async fn run_command(
        &self,
        argv: Vec<String>,
        limits: RunLimits,
        mut stdout: OutputSink,
        mut stderr: OutputSink,
        cancel: CancelFuture,
    ) -> Result<RunOutcome, MemoryError> {
        let url = self.endpoint("/v1/run")?;
        let timeout_ms = u64::try_from(limits.timeout.as_millis()).unwrap_or(u64::MAX);
        let wire = RunRequestWire {
            argv,
            timeout_ms: Some(timeout_ms),
            max_output_bytes: Some(limits.max_output_bytes),
        };
        let payload =
            serde_json::to_vec(&wire).map_err(|error| MemoryError::internal(error.to_string()))?;
        let request_timeout = limits.timeout.saturating_add(RUN_STREAM_TIMEOUT_BUFFER);

        let response = self
            .client
            .post(url)
            .timeout(request_timeout)
            .header(header::CONTENT_TYPE, JSON_CONTENT_TYPE)
            .body(payload)
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        let status = response.status();
        if !status.is_success() {
            let bytes = self.read_bounded(response).await?;
            return Err(parse_error_body(status, &bytes));
        }
        Self::require_content_type(&response, RUN_CONTENT_TYPE)?;

        let byte_stream = response.bytes_stream();
        tokio::pin!(byte_stream);
        tokio::pin!(cancel);
        let mut decoder = FrameDecoder::new();

        loop {
            tokio::select! {
                biased;
                () = &mut cancel => {
                    return Ok(RunOutcome::Cancelled);
                }
                next = byte_stream.next() => {
                    match next {
                        Some(Ok(chunk)) => {
                            decoder.push(&chunk);
                            while let Some(frame) = decoder.next_frame()? {
                                match frame {
                                    RunFrame::Stdout(data) => {
                                        stdout.write_all(&data).await.map_err(MemoryError::from)?;
                                    }
                                    RunFrame::Stderr(data) => {
                                        stderr.write_all(&data).await.map_err(MemoryError::from)?;
                                    }
                                    RunFrame::Terminal(outcome) => {
                                        let _ = stdout.flush().await;
                                        let _ = stderr.flush().await;
                                        return Ok(outcome);
                                    }
                                }
                            }
                        }
                        Some(Err(error)) => return Err(map_transport_error(&error)),
                        None => {
                            return Err(MemoryError::malformed_response(
                                "run stream ended before a terminal frame was received",
                            ));
                        }
                    }
                }
            }
        }
    }
}

fn list_filter_query(filter: &ListFilter, text: Option<String>) -> Vec<(&'static str, String)> {
    let mut query = Vec::new();
    if let Some(under) = &filter.under {
        query.push(("under", under.as_str()));
    }
    if !filter.with_tags.is_empty() {
        query.push(("with-tag", filter.with_tags.join(",")));
    }
    if let Some(limit) = filter.limit {
        query.push(("limit", limit.to_string()));
    }
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        query.push(("text", text));
    }
    query
}

#[cfg(test)]
mod tests {
    use super::HttpMemoryClient;
    use crate::{client::MemoryClient, error::MemoryError, path::PagePath};

    #[tokio::test]
    async fn invalid_uri_reports_unavailable_rather_than_panicking() {
        let client = HttpMemoryClient::new("not a valid uri");
        let error = client
            .read_page(PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}")))
            .await
            .map_or_else(|error| error, |_| panic!("must fail"));
        assert!(matches!(error, MemoryError::Unavailable { .. }));
    }

    #[tokio::test]
    async fn unreachable_host_reports_unavailable() {
        // Port 0 never accepts connections; this deterministically exercises
        // the connect-failure path without depending on external network
        // access or a fixed unused port.
        let client = HttpMemoryClient::new("http://127.0.0.1:0");
        let error = client
            .read_page(PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}")))
            .await
            .map_or_else(|error| error, |_| panic!("must fail"));
        assert!(matches!(error, MemoryError::Unavailable { .. }));
    }
}
