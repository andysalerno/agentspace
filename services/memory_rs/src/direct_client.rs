//! [`DirectMemoryClient`]: an in-process [`MemoryClient`] adapter that
//! delegates directly to a [`MemoryService`], with no serialization step.
//! This is the transport used by every local CLI command.

use async_trait::async_trait;

use crate::{
    client::{CancelFuture, MemoryClient, OutputSink},
    command_runner::{self, RunLimits, RunOutcome},
    error::MemoryError,
    model::{
        CheckReport, LinksReport, ListFilter, MoveOutcome, MovePageRequest, Page, PageSummary,
        QueryRequest, RemovePageRequest, TagCount, WritePageRequest,
    },
    path::PagePath,
    service::MemoryService,
    store::MemoryStore,
};

/// An in-process [`MemoryClient`] over a [`MemoryService`].
pub struct DirectMemoryClient<S: MemoryStore> {
    service: MemoryService<S>,
}

impl<S: MemoryStore> DirectMemoryClient<S> {
    pub const fn new(service: MemoryService<S>) -> Self {
        Self { service }
    }

    #[must_use]
    pub fn store_root(&self) -> &std::path::Path {
        self.service.store().root()
    }
}

#[async_trait]
impl<S: MemoryStore> MemoryClient for DirectMemoryClient<S> {
    async fn write_page(&self, request: WritePageRequest) -> Result<Page, MemoryError> {
        self.service.write_page(request)
    }

    async fn read_page(&self, path: PagePath) -> Result<Page, MemoryError> {
        self.service.read_page(&path)
    }

    async fn move_page(&self, request: MovePageRequest) -> Result<MoveOutcome, MemoryError> {
        self.service.move_page(request)
    }

    async fn remove_page(&self, request: RemovePageRequest) -> Result<(), MemoryError> {
        self.service.remove_page(request)
    }

    async fn list_pages(&self, filter: ListFilter) -> Result<Vec<PageSummary>, MemoryError> {
        self.service.list_pages(&filter)
    }

    async fn query_pages(&self, request: QueryRequest) -> Result<Vec<PageSummary>, MemoryError> {
        self.service.query_pages(&request)
    }

    async fn list_tags(&self) -> Result<Vec<TagCount>, MemoryError> {
        self.service.list_tags()
    }

    async fn links(
        &self,
        path: PagePath,
        include_backlinks: bool,
    ) -> Result<LinksReport, MemoryError> {
        self.service.links(&path, include_backlinks)
    }

    async fn check(&self) -> Result<CheckReport, MemoryError> {
        self.service.check()
    }

    async fn run_command(
        &self,
        argv: Vec<String>,
        limits: RunLimits,
        stdout: OutputSink,
        stderr: OutputSink,
        cancel: CancelFuture,
    ) -> Result<RunOutcome, MemoryError> {
        let root = self.service.store().root().to_path_buf();
        Ok(command_runner::run(&root, &argv, limits, stdout, stderr, cancel).await)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::DirectMemoryClient;
    use crate::{
        client::MemoryClient, fs_store::FilesystemMemoryStore, model::WritePageRequest,
        path::PagePath, service::MemoryService,
    };

    fn client(root: &std::path::Path) -> DirectMemoryClient<FilesystemMemoryStore> {
        let store =
            FilesystemMemoryStore::open(root).unwrap_or_else(|error| panic!("open store: {error}"));
        DirectMemoryClient::new(MemoryService::new(store))
    }

    #[tokio::test]
    async fn write_then_read_through_client() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let client = client(dir.path());
        client
            .write_page(WritePageRequest {
                path: PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}")),
                title: Some("A".to_owned()),
                tags: Some(vec!["x".to_owned()]),
                body: "body".to_owned(),
                overwrite: false,
                expected_revision: None,
                actor: None,
            })
            .await
            .unwrap_or_else(|error| panic!("write: {error}"));

        let page = client
            .read_page(PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}")))
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(page.metadata.title, "A");
    }

    #[tokio::test]
    async fn run_command_executes_in_store_root() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let client = client(dir.path());
        let stdout = Box::new(tokio::io::sink());
        let stderr = Box::new(tokio::io::sink());
        let outcome = client
            .run_command(
                vec!["pwd".to_owned()],
                crate::command_runner::RunLimits::default(),
                stdout,
                stderr,
                Box::pin(std::future::pending()),
            )
            .await
            .unwrap_or_else(|error| panic!("run: {error}"));
        assert_eq!(outcome, crate::command_runner::RunOutcome::Exited(0));
    }
}
