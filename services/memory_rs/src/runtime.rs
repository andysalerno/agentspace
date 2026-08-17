//! Runtime selection and HTTP serving for `agentspace memory`.

use std::{path::PathBuf, sync::Arc, time::Duration};

use tokio::net::TcpListener;

use crate::{
    cli::{self, MemoryArgs},
    client::MemoryClient,
    direct_client::DirectMemoryClient,
    fs_store::FilesystemMemoryStore,
    http_client::HttpMemoryClient,
    server::{self, AppState},
    service::MemoryService,
};

const ENV_MEMORY_URI: &str = "AGENTSPACE_MEMORY_URI";
const ENV_MEMORY_DIR: &str = "AGENTSPACE_MEMORY_DIR";
const MAX_SERVE_REQUEST_BYTES: usize = 4 * 1024 * 1024;

enum Backend {
    Local(PathBuf),
    Remote(String),
}

/// Runs one memory command or the memory HTTP service.
pub async fn run(args: MemoryArgs, json: bool) -> i32 {
    if args.serve {
        return serve(&args).await;
    }

    match resolve_backend(&args) {
        Backend::Local(root) => match FilesystemMemoryStore::open(&root) {
            Ok(store) => {
                let client = DirectMemoryClient::new(MemoryService::new(store));
                cli::run(args, &client as &dyn MemoryClient, json).await
            }
            Err(error) => {
                eprintln!(
                    "agentspace memory: failed to open store at {}: {error}",
                    root.display()
                );
                1
            }
        },
        Backend::Remote(uri) => {
            let client = HttpMemoryClient::new(uri);
            cli::run(args, &client as &dyn MemoryClient, json).await
        }
    }
}

async fn serve(args: &MemoryArgs) -> i32 {
    let root = match resolve_serve_root(args) {
        Ok(root) => root,
        Err(message) => {
            eprintln!("agentspace memory: {message}");
            return 2;
        }
    };

    let store = match FilesystemMemoryStore::open(&root) {
        Ok(store) => store,
        Err(error) => {
            eprintln!(
                "agentspace memory: failed to open store at {}: {error}",
                root.display()
            );
            return 1;
        }
    };

    let client: Arc<dyn MemoryClient> =
        Arc::new(DirectMemoryClient::new(MemoryService::new(store)));
    let app = server::build_router(AppState::new(client), MAX_SERVE_REQUEST_BYTES);

    let listener = match TcpListener::bind((args.host.as_str(), args.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!(
                "agentspace memory: failed to bind {}:{}: {error}",
                args.host, args.port
            );
            return 1;
        }
    };

    let address = listener.local_addr().map_or_else(
        |_| format!("{}:{}", args.host, args.port),
        |address| address.to_string(),
    );
    tracing::info!(%address, root = %root.display(), "agentspace memory --serve listening");

    if let Err(error) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        eprintln!("agentspace memory: server error: {error}");
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
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn resolve_serve_root(args: &MemoryArgs) -> Result<PathBuf, String> {
    if let Some(root) = &args.root {
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

fn resolve_backend(args: &MemoryArgs) -> Backend {
    if let Some(uri) = &args.uri {
        return Backend::Remote(uri.clone());
    }
    if let Some(root) = &args.root {
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

fn built_in_root() -> PathBuf {
    std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".agentspace/memory"),
        |home| PathBuf::from(home).join(".local/share/agentspace/memory"),
    )
}
