//! The `memory` binary: parses CLI arguments, resolves the configured
//! backend, and runs one command through [`memory_rs::client::MemoryClient`].

use std::path::PathBuf;

use clap::Parser as _;
use memory_rs::{
    cli::{self, Cli},
    client::MemoryClient,
    direct_client::DirectMemoryClient,
    fs_store::FilesystemMemoryStore,
    remote_client::RemoteMemoryClient,
    service::MemoryService,
};

const ENV_MEMORY_URI: &str = "AGENTSPACE_MEMORY_URI";
const ENV_MEMORY_DIR: &str = "AGENTSPACE_MEMORY_DIR";

enum Backend {
    Local(PathBuf),
    Remote(String),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.serve {
        eprintln!("memory: --serve is not implemented until milestone 3");
        std::process::exit(5);
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
            let client = RemoteMemoryClient::new(uri);
            cli::run(cli, &client as &dyn MemoryClient).await
        }
    };

    std::process::exit(exit_code);
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
