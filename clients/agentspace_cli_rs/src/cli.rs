use clap::{Parser, Subcommand};
use memory_rs::cli::MemoryArgs;
use serde::Serialize;

use crate::skills::{cli as skills_cli, error::SkillsError};

/// Interact with the `AgentSpace` host environment.
#[derive(Debug, Parser)]
#[command(name = "agentspace", version, about, propagate_version = true)]
pub struct Cli {
    /// Emit stable, machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Recall and maintain durable shared memory.
    Memory(MemoryArgs),
    /// Inspect and maintain reusable `AgentSpace` skills.
    Skills(skills_cli::SkillsArgs),
}

pub async fn run(cli: Cli) -> i32 {
    match cli.command {
        Command::Memory(args) => memory_rs::runtime::run(args, cli.json).await,
        Command::Skills(args) => match skills_cli::run(args, cli.json).await {
            Ok(()) => 0,
            Err(error) => {
                print_skills_error(&error, cli.json);
                error.exit_code()
            }
        },
    }
}

fn print_skills_error(error: &SkillsError, json: bool) {
    if json {
        let payload = JsonError {
            error: JsonErrorBody {
                kind: error.kind(),
                message: error.to_string(),
            },
        };
        match serde_json::to_string(&payload) {
            Ok(text) => eprintln!("{text}"),
            Err(json_error) => {
                eprintln!("agentspace skills: failed to serialize error: {json_error}");
            }
        }
    } else {
        eprintln!("agentspace skills: {error}");
    }
}

#[derive(Serialize)]
struct JsonError {
    error: JsonErrorBody,
}

#[derive(Serialize)]
struct JsonErrorBody {
    kind: &'static str,
    message: String,
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::skills::cli::SkillsCommand;

    #[test]
    fn parses_memory_under_top_level_command() {
        let cli = Cli::try_parse_from([
            "agentspace",
            "memory",
            "--uri",
            "http://memory:8005",
            "query",
            "project",
        ])
        .unwrap_or_else(|error| panic!("parse memory command: {error}"));

        let Command::Memory(args) = cli.command else {
            panic!("expected memory command");
        };
        assert_eq!(args.uri.as_deref(), Some("http://memory:8005"));
        assert!(matches!(
            args.command,
            Some(memory_rs::cli::Commands::Query { text, .. }) if text == "project"
        ));
    }

    #[test]
    fn parses_skills_and_propagates_global_json() {
        let cli = Cli::try_parse_from([
            "agentspace",
            "skills",
            "sync",
            "/workspace/.agentspace-skills/example",
            "--json",
        ])
        .unwrap_or_else(|error| panic!("parse skills command: {error}"));

        assert!(cli.json);
        let Command::Skills(args) = cli.command else {
            panic!("expected skills command");
        };
        assert!(matches!(
            args.command,
            SkillsCommand::Sync { directory }
                if directory == std::path::Path::new(
                    "/workspace/.agentspace-skills/example"
                )
        ));
    }
}
