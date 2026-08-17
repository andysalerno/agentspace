//! The single transport-neutral interface every CLI command calls.
//!
//! [`MemoryClient`] is implemented by
//! [`crate::direct_client::DirectMemoryClient`] in-process and, from
//! milestone 3 onward, by an HTTP transport selected through
//! `AGENTSPACE_MEMORY_URI`.

use std::{future::Future, pin::Pin};

use async_trait::async_trait;
use tokio::io::AsyncWrite;

use crate::{
    command_runner::{RunLimits, RunOutcome},
    error::MemoryError,
    model::{
        CheckReport, LinksReport, ListFilter, MoveOutcome, MovePageRequest, Page, PageSummary,
        QueryRequest, RemovePageRequest, TagCount, WritePageRequest,
    },
    path::PagePath,
};

/// A boxed, `'static` cancellation future accepted by [`MemoryClient::run_command`].
pub type CancelFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
/// A boxed, `'static` output sink accepted by [`MemoryClient::run_command`].
pub type OutputSink = Box<dyn AsyncWrite + Unpin + Send>;

/// The transport-neutral operations backing every `agentspace memory` command.
///
/// Both the in-process `DirectMemoryClient` and any future HTTP client
/// implement exactly this interface; validation, revision/conflict
/// semantics, and link maintenance live behind it in `MemoryService`; a
/// transport implementation must not duplicate that behavior.
#[async_trait]
pub trait MemoryClient: Send + Sync {
    async fn write_page(&self, request: WritePageRequest) -> Result<Page, MemoryError>;

    async fn read_page(&self, path: PagePath) -> Result<Page, MemoryError>;

    async fn move_page(&self, request: MovePageRequest) -> Result<MoveOutcome, MemoryError>;

    async fn remove_page(&self, request: RemovePageRequest) -> Result<(), MemoryError>;

    async fn list_pages(&self, filter: ListFilter) -> Result<Vec<PageSummary>, MemoryError>;

    async fn query_pages(&self, request: QueryRequest) -> Result<Vec<PageSummary>, MemoryError>;

    async fn list_tags(&self) -> Result<Vec<TagCount>, MemoryError>;

    async fn links(
        &self,
        path: PagePath,
        include_backlinks: bool,
    ) -> Result<LinksReport, MemoryError>;

    async fn check(&self) -> Result<CheckReport, MemoryError>;

    /// Runs an allowlisted command, streaming its stdout/stderr into the
    /// given sinks as bytes arrive, and resolves once the child exits, is
    /// terminated by a limit, or `cancel` completes first.
    async fn run_command(
        &self,
        argv: Vec<String>,
        limits: RunLimits,
        stdout: OutputSink,
        stderr: OutputSink,
        cancel: CancelFuture,
    ) -> Result<RunOutcome, MemoryError>;
}
