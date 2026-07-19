//! Shared test scaffolding for the `memory_rs` integration test suites: a
//! tempdir-backed `DirectMemoryClient` and a real Axum server (bound to an
//! ephemeral port) fronted by an `HttpMemoryClient`, so the same behavior
//! can be exercised against both transports.

#![allow(dead_code)]

use std::{
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use memory_rs::{
    client::MemoryClient, direct_client::DirectMemoryClient, fs_store::FilesystemMemoryStore,
    http_client::HttpMemoryClient, server, service::MemoryService,
};
use tokio::{io::AsyncWrite, net::TcpListener, task::JoinHandle};

/// Bounds request bodies in every test server; generous relative to
/// anything a test sends.
const TEST_MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// A `DirectMemoryClient` over a fresh, temporary filesystem store. The
/// `TempDir` must be kept alive for as long as the client is used.
pub fn direct_client() -> (DirectMemoryClient<FilesystemMemoryStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let store = FilesystemMemoryStore::open(dir.path())
        .unwrap_or_else(|error| panic!("open store: {error}"));
    (DirectMemoryClient::new(MemoryService::new(store)), dir)
}

/// A live Axum server (backed by its own temporary filesystem store) bound
/// to `127.0.0.1` on an OS-assigned port, plus an `HttpMemoryClient`
/// pointed at it. Aborts the server task on drop.
pub struct HttpTestServer {
    pub client: HttpMemoryClient,
    pub addr: SocketAddr,
    _dir: tempfile::TempDir,
    handle: JoinHandle<()>,
}

impl Drop for HttpTestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Starts a real server for a fresh temporary store and returns a client
/// pointed at it.
pub async fn http_client() -> HttpTestServer {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let store = FilesystemMemoryStore::open(dir.path())
        .unwrap_or_else(|error| panic!("open store: {error}"));
    let client: Arc<dyn MemoryClient> =
        Arc::new(DirectMemoryClient::new(MemoryService::new(store)));
    let app = server::build_router(server::AppState::new(client), TEST_MAX_REQUEST_BYTES);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("bind: {error}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("local_addr: {error}"));
    let handle = tokio::spawn(async move {
        let _serve_result = axum::serve(listener, app).await;
    });

    HttpTestServer {
        client: HttpMemoryClient::new(format!("http://{addr}")),
        addr,
        _dir: dir,
        handle,
    }
}

/// An in-memory `AsyncWrite` sink usable as a `/v1/run` stdout/stderr
/// target, mirroring the pattern already used by `command_runner`'s own
/// unit tests.
#[derive(Clone, Default)]
pub struct VecSink(Arc<Mutex<Vec<u8>>>);

impl VecSink {
    #[must_use]
    pub fn contents(&self) -> Vec<u8> {
        self.0
            .lock()
            .unwrap_or_else(|error| panic!("lock: {error}"))
            .clone()
    }
}

impl AsyncWrite for VecSink {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.0
            .lock()
            .unwrap_or_else(|error| panic!("lock: {error}"))
            .extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
