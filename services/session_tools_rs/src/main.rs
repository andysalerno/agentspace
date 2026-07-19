use clap::Parser as _;
use session_tools_rs::{Cli, run};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = run(&cli).await;
    if !result.stdout.is_empty() {
        println!("{}", result.stdout);
    }
    if !result.stderr.is_empty() {
        eprintln!("{}", result.stderr);
    }
    std::process::exit(i32::from(result.exit_code));
}
