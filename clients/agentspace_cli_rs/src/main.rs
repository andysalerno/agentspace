use agentspace_cli_rs::cli::{self, Cli};
use clap::Parser as _;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("agentspace_cli_rs=info,memory_rs=info"));
    fmt().with_env_filter(env_filter).init();

    std::process::exit(cli::run(Cli::parse()).await);
}
