use std::error::Error;

use agent_host_rs::{AppConfig, AppState, build_router};
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    init_tracing();

    let config = AppConfig::from_env()?;
    let bind_host = config.bind_host().to_owned();
    let bind_port = config.bind_port();
    let state = AppState::try_new(config)?;
    let app = build_router(state.clone());
    let listener = TcpListener::bind((bind_host.as_str(), bind_port)).await?;

    tracing::info!(
        address = %listener.local_addr()?,
        "starting agent_host_rs"
    );

    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;

    state.shutdown().await;

    result?;

    Ok(())
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("agent_host_rs=info,tower_http=info"));

    fmt().with_env_filter(env_filter).init();
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
        () = ctrl_c => {},
        () = terminate => {},
    }
}
