//! [`RemoteMemoryClient`]: a placeholder [`MemoryClient`] selected when the
//! CLI is configured with `--uri`/`AGENTSPACE_MEMORY_URI`.
//!
//! The HTTP transport is implemented in milestone 3. Until then, every
//! method returns [`MemoryError::NotImplemented`] rather than silently
//! falling back to local storage, matching the plan's requirement that a
//! configured remote backend never has a success-shaped local fallback.

use async_trait::async_trait;

use crate::{
    client::{CancelFuture, MemoryClient, OutputSink},
    command_runner::{RunLimits, RunOutcome},
    error::MemoryError,
    model::{
        CheckReport, LinksReport, ListFilter, MoveOutcome, MovePageRequest, Page, PageSummary,
        QueryRequest, RemovePageRequest, TagCount, WritePageRequest,
    },
    path::PagePath,
};

/// The remote URI a `RemoteMemoryClient` was configured with, kept only for
/// diagnostics until the HTTP transport exists.
pub struct RemoteMemoryClient {
    uri: String,
}

impl RemoteMemoryClient {
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into() }
    }

    fn not_implemented(&self) -> MemoryError {
        MemoryError::not_implemented(format!("the remote memory transport ({})", self.uri))
    }
}

#[async_trait]
impl MemoryClient for RemoteMemoryClient {
    async fn write_page(&self, _request: WritePageRequest) -> Result<Page, MemoryError> {
        Err(self.not_implemented())
    }

    async fn read_page(&self, _path: PagePath) -> Result<Page, MemoryError> {
        Err(self.not_implemented())
    }

    async fn move_page(&self, _request: MovePageRequest) -> Result<MoveOutcome, MemoryError> {
        Err(self.not_implemented())
    }

    async fn remove_page(&self, _request: RemovePageRequest) -> Result<(), MemoryError> {
        Err(self.not_implemented())
    }

    async fn list_pages(&self, _filter: ListFilter) -> Result<Vec<PageSummary>, MemoryError> {
        Err(self.not_implemented())
    }

    async fn query_pages(&self, _request: QueryRequest) -> Result<Vec<PageSummary>, MemoryError> {
        Err(self.not_implemented())
    }

    async fn list_tags(&self) -> Result<Vec<TagCount>, MemoryError> {
        Err(self.not_implemented())
    }

    async fn links(
        &self,
        _path: PagePath,
        _include_backlinks: bool,
    ) -> Result<LinksReport, MemoryError> {
        Err(self.not_implemented())
    }

    async fn check(&self) -> Result<CheckReport, MemoryError> {
        Err(self.not_implemented())
    }

    async fn run_command(
        &self,
        _argv: Vec<String>,
        _limits: RunLimits,
        _stdout: OutputSink,
        _stderr: OutputSink,
        _cancel: CancelFuture,
    ) -> Result<RunOutcome, MemoryError> {
        Err(self.not_implemented())
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteMemoryClient;
    use crate::{client::MemoryClient, error::MemoryError, path::PagePath};

    #[tokio::test]
    async fn every_method_reports_not_implemented() {
        let client = RemoteMemoryClient::new("https://memory.internal");
        let error = client
            .read_page(PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}")))
            .await
            .map_or_else(|error| error, |_| panic!("must be not implemented"));
        assert!(matches!(error, MemoryError::NotImplemented { .. }));
    }
}
