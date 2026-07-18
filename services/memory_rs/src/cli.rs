//! Clap CLI surface for the `memory` binary.
//!
//! Every command constructs a transport-neutral request and calls it
//! through [`crate::client::MemoryClient`] — never directly against
//! [`crate::store::MemoryStore`] or [`crate::service::MemoryService`] — so
//! local and remote invocations behave identically.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::{
    client::MemoryClient,
    command_runner::{RunLimits, RunOutcome},
    error::MemoryError,
    model::{
        ListFilter, MovePageRequest, PageSummary, QueryRequest, RemovePageRequest, WritePageRequest,
    },
    path::PagePath,
};

/// `AgentSpace` text-first memory store and CLI.
#[derive(Debug, Parser)]
#[command(name = "memory", version, about, propagate_version = true)]
pub struct Cli {
    /// Use a local filesystem store at this root instead of the default.
    /// Mutually exclusive with `--uri`. Not part of the agent-facing
    /// contract; operator/local-development use only.
    #[arg(long, global = true, conflicts_with = "uri")]
    pub root: Option<PathBuf>,

    /// Use a remote memory service at this URI instead of local storage.
    /// Mutually exclusive with `--root`.
    #[arg(long, global = true, conflicts_with = "root")]
    pub uri: Option<String>,

    /// Emit stable, machine-readable JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,

    /// Run the Axum HTTP server over the resolved local store instead of a
    /// one-shot command. Mutually exclusive with `--uri`; `--serve` never
    /// serves a remote-configured backend (rejected at runtime if
    /// `AGENTSPACE_MEMORY_URI` is set instead of `--uri`, since `clap`
    /// cannot see environment variables).
    #[arg(long, conflicts_with = "uri")]
    pub serve: bool,

    /// Bind host for `--serve`.
    #[arg(long, default_value = "127.0.0.1", requires = "serve")]
    pub host: String,

    /// Bind port for `--serve`.
    #[arg(long, default_value_t = 8005, requires = "serve")]
    pub port: u16,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Create or replace a page. Reads the body from `--file` or stdin.
    Write {
        path: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        /// Replace an existing page unconditionally.
        #[arg(long)]
        overwrite: bool,
        /// Replace an existing page only if its current revision matches.
        #[arg(long = "if-revision")]
        if_revision: Option<String>,
    },
    /// Print a page's frontmatter and body.
    Read { path: String },
    /// Rename a page, updating other pages' relative links to it.
    Move {
        source: String,
        destination: String,
        #[arg(long = "if-revision")]
        if_revision: Option<String>,
    },
    /// Delete a page.
    Rm {
        path: String,
        #[arg(long = "if-revision")]
        if_revision: Option<String>,
    },
    /// Inspect pages.
    Pages {
        #[command(subcommand)]
        action: PagesAction,
    },
    /// Case-insensitive text query over path, title, tags, and body.
    Query {
        text: String,
        #[command(flatten)]
        filter: FilterArgs,
    },
    /// Inspect tags.
    Tags {
        #[command(subcommand)]
        action: TagsAction,
    },
    /// Show a page's outgoing links, and inbound backlinks with `--backlinks`.
    Links {
        path: String,
        #[arg(long)]
        backlinks: bool,
    },
    /// Report invalid frontmatter, unsafe paths, duplicate tags, and broken
    /// links across the whole store.
    Check,
    /// Run one allowlisted read-oriented executable directly (never through
    /// a shell), with the store root as its working directory.
    Run {
        /// The executable and its arguments, e.g. `rg birthday`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        argv: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum PagesAction {
    /// List page summaries.
    Ls {
        #[command(flatten)]
        filter: FilterArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum TagsAction {
    /// List every normalized tag with its page count.
    Ls,
}

#[derive(Args, Clone, Debug, Default)]
pub struct FilterArgs {
    /// Restrict results to this subtree.
    #[arg(long)]
    pub under: Option<String>,
    /// Restrict results to pages with this tag (repeatable; all must match).
    #[arg(long = "with-tag")]
    pub with_tag: Vec<String>,
    /// Maximum number of results.
    #[arg(long)]
    pub limit: Option<usize>,
}

impl FilterArgs {
    fn into_filter(self) -> Result<ListFilter, MemoryError> {
        let under = self
            .under
            .map(|value| PagePath::parse(&value))
            .transpose()?;
        Ok(ListFilter {
            under,
            with_tags: self.with_tag,
            limit: self.limit,
        })
    }
}

/// A stable machine-readable error envelope for `--json` output.
#[derive(Serialize)]
struct JsonError {
    error: JsonErrorBody,
}

#[derive(Serialize)]
struct JsonErrorBody {
    kind: &'static str,
    message: String,
}

/// Maps a [`MemoryError`] to a process exit code, distinguishing usage
/// errors, missing pages, conflicts, unimplemented features, and internal
/// failures.
#[must_use]
pub const fn exit_code_for(error: &MemoryError) -> i32 {
    match error {
        MemoryError::InvalidPath { .. }
        | MemoryError::InvalidFrontmatter { .. }
        | MemoryError::TooLarge { .. }
        | MemoryError::CommandNotAllowed { .. } => 2,
        MemoryError::NotFound { .. } => 3,
        MemoryError::Conflict { .. } | MemoryError::AlreadyExists { .. } => 4,
        MemoryError::NotImplemented { .. } => 5,
        MemoryError::RunTimedOut => 6,
        MemoryError::RunOutputLimitExceeded => 7,
        MemoryError::RunCancelled => 8,
        MemoryError::RunLaunchFailed { .. } => 9,
        MemoryError::Unavailable { .. } => 10,
        MemoryError::MalformedResponse { .. } => 11,
        MemoryError::Lock { .. }
        | MemoryError::Io { .. }
        | MemoryError::Yaml { .. }
        | MemoryError::Internal { .. } => 1,
    }
}

fn print_error(error: &MemoryError, json: bool) {
    if json {
        let payload = JsonError {
            error: JsonErrorBody {
                kind: error.kind(),
                message: error.to_string(),
            },
        };
        if let Ok(text) = serde_json::to_string(&payload) {
            eprintln!("{text}");
            return;
        }
    }
    eprintln!("memory: {error}");
}

fn print_value<T: Serialize>(value: &T, json: bool, human: impl FnOnce(&T)) {
    if json {
        match serde_json::to_string_pretty(value) {
            Ok(text) => println!("{text}"),
            Err(error) => eprintln!("memory: failed to serialize JSON output: {error}"),
        }
    } else {
        human(value);
    }
}

fn read_body(file: Option<&PathBuf>) -> Result<String, MemoryError> {
    use std::io::Read as _;
    if let Some(path) = file {
        Ok(std::fs::read_to_string(path)?)
    } else {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        Ok(buffer)
    }
}

/// Runs a single parsed CLI invocation against `client`, returning the
/// process exit code.
pub async fn run(cli: Cli, client: &dyn MemoryClient) -> i32 {
    let json = cli.json;
    let Some(command) = cli.command else {
        eprintln!("memory: no command given; see `memory --help`");
        return 2;
    };

    let result = execute(command, client, json).await;
    match result {
        Ok(code) => code,
        Err(error) => {
            print_error(&error, json);
            exit_code_for(&error)
        }
    }
}

async fn execute(
    command: Commands,
    client: &dyn MemoryClient,
    json: bool,
) -> Result<i32, MemoryError> {
    match command {
        Commands::Write {
            path,
            title,
            tags,
            file,
            overwrite,
            if_revision,
        } => {
            write(
                client,
                json,
                path,
                title,
                tags,
                file.as_ref(),
                overwrite,
                if_revision,
            )
            .await
        }
        Commands::Read { path } => read(client, json, path).await,
        Commands::Move {
            source,
            destination,
            if_revision,
        } => move_page(client, json, source, destination, if_revision).await,
        Commands::Rm { path, if_revision } => remove(client, json, path, if_revision).await,
        Commands::Pages {
            action: PagesAction::Ls { filter },
        } => list_pages(client, json, filter).await,
        Commands::Query { text, filter } => query(client, json, text, filter).await,
        Commands::Tags {
            action: TagsAction::Ls,
        } => list_tags(client, json).await,
        Commands::Links { path, backlinks } => links(client, json, path, backlinks).await,
        Commands::Check => check(client, json).await,
        Commands::Run { argv } => run_command(client, argv).await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn write(
    client: &dyn MemoryClient,
    json: bool,
    path: String,
    title: Option<String>,
    tags: Vec<String>,
    file: Option<&PathBuf>,
    overwrite: bool,
    if_revision: Option<String>,
) -> Result<i32, MemoryError> {
    let page_path = PagePath::parse(&path)?;
    let body = read_body(file)?;
    let page = client
        .write_page(WritePageRequest {
            path: page_path,
            title,
            tags: if tags.is_empty() { None } else { Some(tags) },
            body,
            overwrite,
            expected_revision: if_revision,
            actor: std::env::var("AGENTSPACE_AGENT_ID").ok(),
        })
        .await?;

    print_value(&page_summary_json(&page), json, |_| {
        println!("wrote {} (revision {})", page.path, page.revision);
    });
    Ok(0)
}

#[derive(Serialize)]
struct WrittenPage {
    path: String,
    title: String,
    tags: Vec<String>,
    revision: String,
}

fn page_summary_json(page: &crate::model::Page) -> WrittenPage {
    WrittenPage {
        path: page.path.as_str(),
        title: page.metadata.title.clone(),
        tags: page.metadata.tags.clone(),
        revision: page.revision.0.clone(),
    }
}

#[derive(Serialize)]
struct ReadView<'a> {
    path: &'a str,
    title: &'a str,
    tags: &'a [String],
    revision: &'a str,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    body: &'a str,
    outgoing_links: &'a [crate::model::PageLink],
}

async fn read(client: &dyn MemoryClient, json: bool, path: String) -> Result<i32, MemoryError> {
    let page_path = PagePath::parse(&path)?;
    let page = client.read_page(page_path.clone()).await?;

    if json {
        let links = client.links(page_path, false).await?;
        let path_str = page.path.as_str();
        print_value(
            &ReadView {
                path: &path_str,
                title: &page.metadata.title,
                tags: &page.metadata.tags,
                revision: &page.revision.0,
                created_at: page.metadata.created_at,
                updated_at: page.metadata.updated_at,
                body: &page.body,
                outgoing_links: &links.outgoing,
            },
            true,
            |_| {},
        );
    } else {
        println!("# {} ({})", page.metadata.title, page.path);
        println!("tags: {}", page.metadata.tags.join(", "));
        println!("revision: {}", page.revision);
        println!();
        print!("{}", page.body);
    }
    Ok(0)
}

async fn move_page(
    client: &dyn MemoryClient,
    json: bool,
    source: String,
    destination: String,
    if_revision: Option<String>,
) -> Result<i32, MemoryError> {
    let outcome = client
        .move_page(MovePageRequest {
            source: PagePath::parse(&source)?,
            destination: PagePath::parse(&destination)?,
            expected_revision: if_revision,
            actor: std::env::var("AGENTSPACE_AGENT_ID").ok(),
        })
        .await?;

    print_value(&outcome, json, |outcome| {
        println!("moved {} -> {}", outcome.source, outcome.destination);
        if !outcome.updated_referrers.is_empty() {
            println!("updated links in: {}", outcome.updated_referrers.join(", "));
        }
    });
    Ok(0)
}

async fn remove(
    client: &dyn MemoryClient,
    json: bool,
    path: String,
    if_revision: Option<String>,
) -> Result<i32, MemoryError> {
    let page_path = PagePath::parse(&path)?;
    client
        .remove_page(RemovePageRequest {
            path: page_path.clone(),
            expected_revision: if_revision,
        })
        .await?;

    print_value(
        &serde_json::json!({ "removed": page_path.as_str() }),
        json,
        |_| {
            println!("removed {path}");
        },
    );
    Ok(0)
}

async fn list_pages(
    client: &dyn MemoryClient,
    json: bool,
    filter: FilterArgs,
) -> Result<i32, MemoryError> {
    let pages = client.list_pages(filter.into_filter()?).await?;
    print_summaries(&pages, json);
    Ok(0)
}

async fn query(
    client: &dyn MemoryClient,
    json: bool,
    text: String,
    filter: FilterArgs,
) -> Result<i32, MemoryError> {
    let pages = client
        .query_pages(QueryRequest {
            text,
            filter: filter.into_filter()?,
        })
        .await?;
    print_summaries(&pages, json);
    Ok(0)
}

fn print_summaries(pages: &[PageSummary], json: bool) {
    print_value(&pages.to_vec(), json, |pages| {
        if pages.is_empty() {
            println!("(no pages)");
            return;
        }
        for page in pages {
            println!("{}\t{}\t[{}]", page.path, page.title, page.tags.join(", "));
        }
    });
}

async fn list_tags(client: &dyn MemoryClient, json: bool) -> Result<i32, MemoryError> {
    let tags = client.list_tags().await?;
    print_value(&tags, json, |tags| {
        if tags.is_empty() {
            println!("(no tags)");
            return;
        }
        for tag in tags {
            println!("{}\t{}", tag.tag, tag.count);
        }
    });
    Ok(0)
}

async fn links(
    client: &dyn MemoryClient,
    json: bool,
    path: String,
    backlinks: bool,
) -> Result<i32, MemoryError> {
    let page_path = PagePath::parse(&path)?;
    let report = client.links(page_path, backlinks).await?;
    print_value(&report, json, |report| {
        println!("outgoing links for {}:", report.path);
        if report.outgoing.is_empty() {
            println!("  (none)");
        }
        for link in &report.outgoing {
            let target = link.resolved_path.as_deref().unwrap_or(&link.raw_target);
            let status = if link.broken { " (broken)" } else { "" };
            println!("  [{}]({}){status}", link.text, target);
        }
        if backlinks {
            println!("backlinks:");
            if report.backlinks.is_empty() {
                println!("  (none)");
            }
            for backlink in &report.backlinks {
                println!(
                    "  {} -> [{}]({})",
                    backlink.from, backlink.text, backlink.raw_target
                );
            }
        }
    });
    Ok(0)
}

async fn check(client: &dyn MemoryClient, json: bool) -> Result<i32, MemoryError> {
    let report = client.check().await?;
    print_value(&report, json, |report| {
        if report.is_clean() {
            println!("no issues found");
            return;
        }
        for issue in &report.issues {
            match &issue.path {
                Some(path) => println!("{path}: {}", issue.message),
                None => println!("{}", issue.message),
            }
        }
    });
    // `check` reports findings without treating them as a command failure;
    // a non-zero exit still distinguishes "issues found" from "clean".
    Ok(i32::from(!report.is_clean()))
}

async fn run_command(client: &dyn MemoryClient, argv: Vec<String>) -> Result<i32, MemoryError> {
    if argv.is_empty() {
        return Err(MemoryError::command_not_allowed(String::new()));
    }
    let stdout: crate::client::OutputSink = Box::new(tokio::io::stdout());
    let stderr: crate::client::OutputSink = Box::new(tokio::io::stderr());
    let cancel: crate::client::CancelFuture = Box::pin(ctrl_c());

    let outcome = client
        .run_command(argv, RunLimits::default(), stdout, stderr, cancel)
        .await?;

    match outcome {
        RunOutcome::Exited(code) => Ok(code),
        RunOutcome::TimedOut => Err(MemoryError::RunTimedOut),
        RunOutcome::OutputLimitExceeded => Err(MemoryError::RunOutputLimitExceeded),
        RunOutcome::Cancelled => Err(MemoryError::RunCancelled),
        RunOutcome::NotAllowed(command) => Err(MemoryError::command_not_allowed(command)),
        RunOutcome::LaunchFailed(message) => Err(MemoryError::run_launch_failed(message)),
    }
}

async fn ctrl_c() {
    let _result = tokio::signal::ctrl_c().await;
}
