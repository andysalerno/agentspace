//! The Axum HTTP adapter for `memory --serve`.
//!
//! Every handler calls through [`MemoryClient`] -- never directly against
//! [`crate::service::MemoryService`] or [`crate::store::MemoryStore`] -- the
//! same rule [`crate::cli`] follows, so this adapter can front any
//! transport-neutral client (in production, always a local
//! [`crate::direct_client::DirectMemoryClient`]; in the contract test suite,
//! also an [`crate::http_client::HttpMemoryClient`] pointed at another
//! instance of this very router).

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bytes::Bytes;
use tokio::{io::AsyncReadExt as _, sync::mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::{timeout::TimeoutLayer, trace::TraceLayer};

use crate::{
    client::{CancelFuture, MemoryClient, OutputSink},
    command_runner::{self, RunLimits, RunOutcome},
    error::MemoryError,
    model::{ListFilter, MovePageRequest, QueryRequest, RemovePageRequest, WritePageRequest},
    path::PagePath,
    run_stream::{self, RUN_CONTENT_TYPE},
    wire::{
        ContentQuery, ErrorEnvelope, LinksQuery, ListPagesQuery, MovePageWire, PageWire,
        RunRequestWire, WritePageWire,
    },
};

/// Default execution-time limit applied to `/v1/run` when the caller does
/// not request a smaller one, and the ceiling no caller-requested timeout
/// may exceed.
const MAX_RUN_TIMEOUT: Duration = Duration::from_mins(2);
/// Default and maximum total stdout+stderr bytes streamed by one `/v1/run`
/// invocation.
const MAX_RUN_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// The size of each stdout/stderr pipe standing between the child and the
/// HTTP body stream, bounding how far the child can run ahead of a slow
/// client before backpressure reaches it (and, transitively, the child).
const STREAM_PIPE_CAPACITY: usize = 64 * 1024;
/// The bounded channel depth carrying encoded frames to the HTTP body.
const FRAME_CHANNEL_CAPACITY: usize = 8;
/// Bounds every request end-to-end, comfortably above the largest possible
/// `/v1/run` duration (`MAX_RUN_TIMEOUT` plus encode/flush overhead) so a
/// legitimate long-running command is never cut off, while still bounding
/// a request stuck for other reasons (e.g. store lock contention).
const REQUEST_TIMEOUT: Duration = Duration::from_mins(3);

/// The shared state behind every handler: a transport-neutral
/// [`MemoryClient`], type-erased so this adapter never depends on a
/// concrete store or transport.
#[derive(Clone)]
pub struct AppState {
    client: Arc<dyn MemoryClient>,
}

impl AppState {
    #[must_use]
    pub fn new(client: Arc<dyn MemoryClient>) -> Self {
        Self { client }
    }
}

/// Builds the `/healthz` and `/v1/...` router documented in
/// `MEMORY_PLAN.md`.
///
/// `max_request_bytes` bounds every request body (`/v1/run`'s JSON launch
/// request included; the resulting streamed response is unrelated to this
/// limit).
pub fn build_router(state: AppState, max_request_bytes: usize) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/pages", get(list_pages))
        .route(
            "/v1/pages/content",
            get(read_page).put(write_page).delete(remove_page),
        )
        .route("/v1/pages/move", post(move_page))
        .route("/v1/tags", get(list_tags))
        .route("/v1/links", get(links))
        .route("/v1/check", get(check))
        .route("/v1/run", post(run))
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(DefaultBodyLimit::max(max_request_bytes))
        .with_state(state)
}

/// Wraps a [`MemoryError`] so it can be returned directly from a handler;
/// maps it to the matching HTTP status and the stable
/// [`ErrorEnvelope`] JSON body.
struct AppError(MemoryError);

impl From<MemoryError> for AppError {
    fn from(error: MemoryError) -> Self {
        Self(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = status_for(&self.0);
        tracing::warn!(kind = self.0.kind(), status = %status, error = %self.0, "memory request failed");
        (status, Json(ErrorEnvelope::from_error(&self.0))).into_response()
    }
}

/// Maps a [`MemoryError`] to the HTTP status of the response reporting it,
/// distinguishing validation, missing, conflict, storage-unavailable, and
/// internal-failure categories.
const fn status_for(error: &MemoryError) -> StatusCode {
    match error {
        MemoryError::InvalidPath { .. }
        | MemoryError::InvalidFrontmatter { .. }
        | MemoryError::TooLarge { .. }
        | MemoryError::CommandNotAllowed { .. } => StatusCode::BAD_REQUEST,
        MemoryError::NotFound { .. } => StatusCode::NOT_FOUND,
        MemoryError::Conflict { .. } | MemoryError::AlreadyExists { .. } => StatusCode::CONFLICT,
        MemoryError::NotImplemented { .. } => StatusCode::NOT_IMPLEMENTED,
        // `Unavailable` is never produced by this adapter itself; only a
        // remote transport synthesizes it. Mapped alongside `Lock` for
        // completeness in case it is ever bubbled here.
        MemoryError::Lock { .. } | MemoryError::Unavailable { .. } => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        // `RunTimedOut`/`RunOutputLimitExceeded`/`RunCancelled`/
        // `RunLaunchFailed` are terminal `/v1/run` *stream* outcomes, never
        // surfaced as a top-level error response by this adapter.
        // `MalformedResponse` is a client-only concept (never emitted by
        // this server). All are mapped defensively in case a future caller
        // of `status_for` reaches them directly.
        MemoryError::RunTimedOut
        | MemoryError::RunOutputLimitExceeded
        | MemoryError::RunCancelled
        | MemoryError::RunLaunchFailed { .. }
        | MemoryError::MalformedResponse { .. }
        | MemoryError::Internal { .. }
        | MemoryError::Io { .. }
        | MemoryError::Yaml { .. } => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn list_pages(
    State(state): State<AppState>,
    Query(query): Query<ListPagesQuery>,
) -> Result<Response, AppError> {
    let under = query
        .under
        .as_deref()
        .map(PagePath::parse)
        .transpose()
        .map_err(AppError::from)?;
    let filter = ListFilter {
        under,
        with_tags: query.tags(),
        limit: query.limit,
    };
    let pages = if let Some(text) = query.text.filter(|text| !text.is_empty()) {
        state
            .client
            .query_pages(QueryRequest { text, filter })
            .await?
    } else {
        state.client.list_pages(filter).await?
    };
    Ok((StatusCode::OK, Json(pages)).into_response())
}

async fn read_page(
    State(state): State<AppState>,
    Query(query): Query<ContentQuery>,
) -> Result<Response, AppError> {
    let path = PagePath::parse(&query.path)?;
    let page = state.client.read_page(path.clone()).await?;
    let links_report = state.client.links(path, false).await?;
    let wire = PageWire::from_page(&page, links_report.outgoing);
    Ok((StatusCode::OK, Json(wire)).into_response())
}

async fn write_page(
    State(state): State<AppState>,
    Query(query): Query<ContentQuery>,
    Json(body): Json<WritePageWire>,
) -> Result<Response, AppError> {
    let path = PagePath::parse(&query.path)?;
    let page = state
        .client
        .write_page(WritePageRequest {
            path,
            title: body.title,
            tags: body.tags,
            body: body.body,
            overwrite: body.overwrite,
            expected_revision: body.expected_revision,
            actor: body.actor,
        })
        .await?;
    let links_report = state.client.links(page.path.clone(), false).await?;
    let wire = PageWire::from_page(&page, links_report.outgoing);
    Ok((StatusCode::OK, Json(wire)).into_response())
}

async fn remove_page(
    State(state): State<AppState>,
    Query(query): Query<ContentQuery>,
) -> Result<StatusCode, AppError> {
    let path = PagePath::parse(&query.path)?;
    state
        .client
        .remove_page(RemovePageRequest {
            path,
            expected_revision: query.expected_revision,
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn move_page(
    State(state): State<AppState>,
    Json(body): Json<MovePageWire>,
) -> Result<Response, AppError> {
    let source = PagePath::parse(&body.source)?;
    let destination = PagePath::parse(&body.destination)?;
    let outcome = state
        .client
        .move_page(MovePageRequest {
            source,
            destination,
            expected_revision: body.expected_revision,
            actor: body.actor,
        })
        .await?;
    Ok((StatusCode::OK, Json(outcome)).into_response())
}

async fn list_tags(State(state): State<AppState>) -> Result<Response, AppError> {
    let tags = state.client.list_tags().await?;
    Ok((StatusCode::OK, Json(tags)).into_response())
}

async fn links(
    State(state): State<AppState>,
    Query(query): Query<LinksQuery>,
) -> Result<Response, AppError> {
    let path = PagePath::parse(&query.path)?;
    let report = state.client.links(path, query.backlinks).await?;
    Ok((StatusCode::OK, Json(report)).into_response())
}

async fn check(State(state): State<AppState>) -> Result<Response, AppError> {
    let report = state.client.check().await?;
    Ok((StatusCode::OK, Json(report)).into_response())
}

/// Handles `POST /v1/run`.
///
/// A disallowed command is rejected immediately as an ordinary JSON error,
/// before any bytes would be streamed. Otherwise this always opens the
/// framed [`crate::run_stream`] response and delegates the launch itself to
/// [`MemoryClient::run_command`], which streams stdout/stderr through a
/// pair of bounded pipes into the HTTP body as they arrive and reports a
/// launch failure as a terminal stream frame rather than a top-level error,
/// matching `MEMORY_PLAN.md`.
async fn run(
    State(state): State<AppState>,
    Json(body): Json<RunRequestWire>,
) -> Result<Response, AppError> {
    if body
        .argv
        .first()
        .is_none_or(|program| !command_runner::is_allowed(program))
    {
        let command = body.argv.first().cloned().unwrap_or_default();
        return Err(MemoryError::command_not_allowed(command).into());
    }

    let timeout = body
        .timeout_ms
        .map_or_else(|| RunLimits::default().timeout, Duration::from_millis)
        .min(MAX_RUN_TIMEOUT);
    let max_output_bytes = body
        .max_output_bytes
        .unwrap_or_else(|| RunLimits::default().max_output_bytes)
        .min(MAX_RUN_OUTPUT_BYTES);
    let limits = RunLimits {
        timeout,
        max_output_bytes,
    };

    let (stdout_write, stdout_read) = tokio::io::duplex(STREAM_PIPE_CAPACITY);
    let (stderr_write, stderr_read) = tokio::io::duplex(STREAM_PIPE_CAPACITY);
    let (frame_tx, frame_rx) =
        mpsc::channel::<Result<Bytes, std::io::Error>>(FRAME_CHANNEL_CAPACITY);

    let cancel_tx = frame_tx.clone();
    let cancel: CancelFuture = Box::pin(async move { cancel_tx.closed().await });
    let client = Arc::clone(&state.client);
    let argv = body.argv;
    let stdout_sink: OutputSink = Box::new(stdout_write);
    let stderr_sink: OutputSink = Box::new(stderr_write);

    tokio::spawn(drive_run(
        client,
        argv,
        limits,
        stdout_sink,
        stderr_sink,
        cancel,
        stdout_read,
        stderr_read,
        frame_tx,
    ));

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, RUN_CONTENT_TYPE)
        .body(Body::from_stream(ReceiverStream::new(frame_rx)))
        .map_err(|error| AppError(MemoryError::internal(error.to_string())))
}

/// Runs one already-validated `/v1/run` invocation to completion in the
/// background, forwarding framed stdout/stderr chunks and, finally, exactly
/// one terminal frame, into `frame_tx`.
#[allow(clippy::too_many_arguments)]
async fn drive_run(
    client: Arc<dyn MemoryClient>,
    argv: Vec<String>,
    limits: RunLimits,
    stdout_sink: OutputSink,
    stderr_sink: OutputSink,
    cancel: CancelFuture,
    stdout_read: tokio::io::DuplexStream,
    stderr_read: tokio::io::DuplexStream,
    frame_tx: mpsc::Sender<Result<Bytes, std::io::Error>>,
) {
    let stdout_forward = tokio::spawn(forward_frames(
        stdout_read,
        run_stream::TAG_STDOUT,
        frame_tx.clone(),
    ));
    let stderr_forward = tokio::spawn(forward_frames(
        stderr_read,
        run_stream::TAG_STDERR,
        frame_tx.clone(),
    ));

    let outcome = match client
        .run_command(argv, limits, stdout_sink, stderr_sink, cancel)
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::warn!(%error, "memory run_command reported a transport failure");
            RunOutcome::LaunchFailed(error.to_string())
        }
    };

    let _stdout_forward_result = stdout_forward.await;
    let _stderr_forward_result = stderr_forward.await;

    let terminal = run_stream::encode_terminal(&outcome);
    let _send_result = frame_tx.send(Ok(Bytes::from(terminal))).await;
}

/// Reads one duplex stream half to EOF, forwarding each chunk read as one
/// framed [`run_stream`] chunk. Ends silently once the writer half is
/// dropped (the child's own output pipe closed) or the receiving HTTP body
/// is gone.
async fn forward_frames(
    mut reader: tokio::io::DuplexStream,
    tag: u8,
    sender: mpsc::Sender<Result<Bytes, std::io::Error>>,
) {
    let mut buffer = [0_u8; 8192];
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let frame = run_stream::encode_chunk(tag, &buffer[..read]);
        if sender.send(Ok(Bytes::from(frame))).await.is_err() {
            break;
        }
    }
}
