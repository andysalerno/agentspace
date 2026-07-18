//! The `memory` binary: parses CLI arguments and either runs a single
//! command through [`memory_rs::client::MemoryClient`] or, with `--serve`,
//! runs the Axum HTTP adapter over a local store.

use std::{path::PathBuf, sync::Arc, time::Duration};

use clap::Parser as _;
use memory_rs::{
    cli::{self, Cli},
    client::MemoryClient,
    direct_client::DirectMemoryClient,
    fs_store::FilesystemMemoryStore,
    http_client::HttpMemoryClient,
    server::{self, AppState},
    service::MemoryService,
};
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, fmt};

const ENV_MEMORY_URI: &str = "AGENTSPACE_MEMORY_URI";
const ENV_MEMORY_DIR: &str = "AGENTSPACE_MEMORY_DIR";
/// Bounds every `--serve` request body, `/v1/run`'s JSON launch request
/// included (the streamed run response itself is unrelated to this limit).
const MAX_SERVE_REQUEST_BYTES: usize = 4 * 1024 * 1024;

enum Backend {
    Local(PathBuf),
    Remote(String),
}

#[tokio::main]
async fn main() {
    init_tracing();

    let cli = Cli::parse();

    if cli.serve {
        std::process::exit(serve(&cli).await);
    }

    let backend = resolve_backend(&cli);
    let exit_code = match backend {
        Backend::Local(root) => match FilesystemMemoryStore::open(&root) {
            Ok(store) => {
                let client = DirectMemoryClient::new(MemoryService::new(store));
                cli::run(cli, &client as &dyn MemoryClient).await
            }
            Err(error) => {
                eprintln!(
                    "memory: failed to open store at {}: {error}",
                    root.display()
                );
                1
            }
        },
        Backend::Remote(uri) => {
            let client = HttpMemoryClient::new(uri);
            cli::run(cli, &client as &dyn MemoryClient).await
        }
    };

    std::process::exit(exit_code);
}

fn init_tracing() {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("memory_rs=info"));
    fmt().with_env_filter(env_filter).init();
}

/// Runs `memory --serve`, returning the process exit code.
///
/// Always serves the resolved *local* root; a remote URI configured via
/// `--uri` (rejected by `clap` itself, see [`Cli`]'s `conflicts_with`) or
/// `AGENTSPACE_MEMORY_URI` is treated as a configuration error rather than
/// silently ignored, per `MEMORY_PLAN.md`'s requirement that `--serve`
/// never proxies to another remote memory service.
async fn serve(cli: &Cli) -> i32 {
    let root = match resolve_serve_root(cli) {
        Ok(root) => root,
        Err(message) => {
            eprintln!("memory: {message}");
            return 2;
        }
    };

    let store = match FilesystemMemoryStore::open(&root) {
        Ok(store) => store,
        Err(error) => {
            eprintln!(
                "memory: failed to open store at {}: {error}",
                root.display()
            );
            return 1;
        }
    };

    let client: Arc<dyn MemoryClient> =
        Arc::new(DirectMemoryClient::new(MemoryService::new(store)));
    let app = server::build_router(AppState::new(client), MAX_SERVE_REQUEST_BYTES);

    let listener = match TcpListener::bind((cli.host.as_str(), cli.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("memory: failed to bind {}:{}: {error}", cli.host, cli.port);
            return 1;
        }
    };

    let address = listener.local_addr().map_or_else(
        |_| format!("{}:{}", cli.host, cli.port),
        |address| address.to_string(),
    );
    tracing::info!(%address, root = %root.display(), "memory --serve listening");

    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        eprintln!("memory: server error: {error}");
        return 1;
    }

    0
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "failed to install Ctrl+C signal handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "failed to install terminate signal handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    // Give in-flight `/v1/run` streams a moment to observe cancellation and
    // report a terminal frame rather than being hard-killed mid-response.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Resolves the local root `--serve` must use: `--root`, then
/// `AGENTSPACE_MEMORY_DIR`, then the private built-in root. Returns `Err`
/// if `AGENTSPACE_MEMORY_URI` is set without `--root`, since serving over a
/// remote-configured environment would otherwise silently ignore the
/// operator's evident intent to use a remote store.
fn resolve_serve_root(cli: &Cli) -> Result<PathBuf, String> {
    if let Some(root) = &cli.root {
        return Ok(root.clone());
    }
    if let Ok(uri) = std::env::var(ENV_MEMORY_URI)
        && !uri.is_empty()
    {
        return Err(format!(
            "--serve requires a local store, but {ENV_MEMORY_URI} is set to {uri:?}; \
             pass --root or set {ENV_MEMORY_DIR} instead, or unset {ENV_MEMORY_URI}"
        ));
    }
    if let Ok(dir) = std::env::var(ENV_MEMORY_DIR)
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    Ok(built_in_root())
}

/// Resolves the backend using the precedence documented in
/// `MEMORY_PLAN.md`: explicit `--uri`/`--root`, then
/// `AGENTSPACE_MEMORY_URI`, then `AGENTSPACE_MEMORY_DIR`, then the private
/// built-in local root.
fn resolve_backend(cli: &Cli) -> Backend {
    if let Some(uri) = &cli.uri {
        return Backend::Remote(uri.clone());
    }
    if let Some(root) = &cli.root {
        return Backend::Local(root.clone());
    }
    if let Ok(uri) = std::env::var(ENV_MEMORY_URI)
        && !uri.is_empty()
    {
        return Backend::Remote(uri);
    }
    if let Ok(dir) = std::env::var(ENV_MEMORY_DIR)
        && !dir.is_empty()
    {
        return Backend::Local(PathBuf::from(dir));
    }
    Backend::Local(built_in_root())
}

/// The private built-in local root used when no backend is configured.
/// Not part of the stable agent-facing contract.
fn built_in_root() -> PathBuf {
    std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".agentspace/memory"),
        |home| PathBuf::from(home).join(".local/share/agentspace/memory"),
    )
}
