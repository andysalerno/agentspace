//! The reusable `MemoryClient` contract: every behavior below is exercised
//! once against an in-process `DirectMemoryClient` and once against a live
//! `HttpMemoryClient` talking to a real Axum server, so both transports are
//! held to the exact same behavior per `MEMORY_PLAN.md`.
//!
//! `run_command`'s "first argument not allowlisted" case is intentionally
//! **not** shared here: `MEMORY_PLAN.md` specifies that `/v1/run` validates
//! the executable allowlist before opening a stream, so the HTTP transport
//! reports it as an ordinary `MemoryError::CommandNotAllowed` response,
//! while `DirectMemoryClient` (matching `command_runner`'s pre-milestone-3
//! behavior) reports it as `Ok(RunOutcome::NotAllowed(..))`. Both are
//! covered by their own transport-specific assertions below.

mod support;

use memory_rs::{
    client::MemoryClient,
    command_runner::{RunLimits, RunOutcome},
    error::MemoryError,
    model::{ListFilter, MovePageRequest, QueryRequest, RemovePageRequest, WritePageRequest},
    path::PagePath,
};
use support::VecSink;

fn path(raw: &str) -> PagePath {
    PagePath::parse(raw).unwrap_or_else(|error| panic!("valid path {raw:?}: {error}"))
}

fn write_request(raw_path: &str, title: &str, tags: &[&str], body: &str) -> WritePageRequest {
    WritePageRequest {
        path: path(raw_path),
        title: Some(title.to_owned()),
        tags: Some(tags.iter().map(|tag| (*tag).to_owned()).collect()),
        body: body.to_owned(),
        overwrite: false,
        expected_revision: None,
        actor: Some("contract-test".to_owned()),
    }
}

async fn write_then_read_round_trip(client: &dyn MemoryClient) {
    let written = client
        .write_page(write_request("notes/plan", "Plan", &["x", "y"], "Hello\n"))
        .await
        .unwrap_or_else(|error| panic!("write: {error}"));
    assert_eq!(written.metadata.title, "Plan");
    assert_eq!(written.metadata.tags, vec!["x".to_owned(), "y".to_owned()]);
    assert!(!written.revision.0.is_empty());

    let read = client
        .read_page(path("notes/plan"))
        .await
        .unwrap_or_else(|error| panic!("read: {error}"));
    assert_eq!(read.body, "Hello\n");
    assert_eq!(read.revision, written.revision);
}

#[tokio::test]
async fn direct_write_then_read_round_trip() {
    let (client, _dir) = support::direct_client();
    write_then_read_round_trip(&client).await;
}

#[tokio::test]
async fn http_write_then_read_round_trip() {
    let server = support::http_client().await;
    write_then_read_round_trip(&server.client).await;
}

async fn write_with_stale_expected_revision_conflicts(client: &dyn MemoryClient) {
    client
        .write_page(write_request("notes/plan", "Plan", &[], "v1\n"))
        .await
        .unwrap_or_else(|error| panic!("initial write: {error}"));

    let error = client
        .write_page(WritePageRequest {
            path: path("notes/plan"),
            title: Some("Plan".to_owned()),
            tags: None,
            body: "v2\n".to_owned(),
            overwrite: false,
            expected_revision: Some("not-the-real-revision".to_owned()),
            actor: None,
        })
        .await
        .map_or_else(
            |error| error,
            |page| panic!("expected conflict, got {page:?}"),
        );
    assert!(matches!(error, MemoryError::Conflict { .. }), "{error:?}");
}

#[tokio::test]
async fn direct_write_with_stale_expected_revision_conflicts() {
    let (client, _dir) = support::direct_client();
    write_with_stale_expected_revision_conflicts(&client).await;
}

#[tokio::test]
async fn http_write_with_stale_expected_revision_conflicts() {
    let server = support::http_client().await;
    write_with_stale_expected_revision_conflicts(&server.client).await;
}

async fn move_page_updates_referrer_links(client: &dyn MemoryClient) {
    client
        .write_page(write_request(
            "projects/agentspace",
            "AgentSpace",
            &[],
            "the project",
        ))
        .await
        .unwrap_or_else(|error| panic!("write target: {error}"));
    client
        .write_page(write_request(
            "people/alice",
            "Alice",
            &[],
            "Related: [AgentSpace](../projects/agentspace.md)",
        ))
        .await
        .unwrap_or_else(|error| panic!("write referrer: {error}"));

    let outcome = client
        .move_page(MovePageRequest {
            source: path("projects/agentspace"),
            destination: path("projects/renamed"),
            expected_revision: None,
            actor: None,
        })
        .await
        .unwrap_or_else(|error| panic!("move: {error}"));
    assert_eq!(outcome.updated_referrers, vec!["people/alice".to_owned()]);

    let referrer = client
        .read_page(path("people/alice"))
        .await
        .unwrap_or_else(|error| panic!("read referrer: {error}"));
    assert!(referrer.body.contains("../projects/renamed.md"));
}

#[tokio::test]
async fn direct_move_page_updates_referrer_links() {
    let (client, _dir) = support::direct_client();
    move_page_updates_referrer_links(&client).await;
}

#[tokio::test]
async fn http_move_page_updates_referrer_links() {
    let server = support::http_client().await;
    move_page_updates_referrer_links(&server.client).await;
}

async fn list_query_and_tags(client: &dyn MemoryClient) {
    client
        .write_page(write_request(
            "a/one",
            "One",
            &["shared", "only-one"],
            "alpha body",
        ))
        .await
        .unwrap_or_else(|error| panic!("write one: {error}"));
    client
        .write_page(write_request("b/two", "Two", &["shared"], "beta body"))
        .await
        .unwrap_or_else(|error| panic!("write two: {error}"));

    let under_a = client
        .list_pages(ListFilter {
            under: Some(path("a")),
            with_tags: Vec::new(),
            limit: None,
        })
        .await
        .unwrap_or_else(|error| panic!("list under a: {error}"));
    assert_eq!(under_a.len(), 1);
    assert_eq!(under_a[0].path, "a/one");

    let shared = client
        .list_pages(ListFilter {
            under: None,
            with_tags: vec!["shared".to_owned()],
            limit: None,
        })
        .await
        .unwrap_or_else(|error| panic!("list shared: {error}"));
    assert_eq!(shared.len(), 2);

    let query_hits = client
        .query_pages(QueryRequest {
            text: "alpha".to_owned(),
            filter: ListFilter::default(),
        })
        .await
        .unwrap_or_else(|error| panic!("query: {error}"));
    assert_eq!(query_hits.len(), 1);
    assert_eq!(query_hits[0].path, "a/one");

    let tags = client
        .list_tags()
        .await
        .unwrap_or_else(|error| panic!("list tags: {error}"));
    let shared_count = tags
        .iter()
        .find(|tag| tag.tag == "shared")
        .unwrap_or_else(|| panic!("shared tag present"));
    assert_eq!(shared_count.count, 2);
}

#[tokio::test]
async fn direct_list_query_and_tags() {
    let (client, _dir) = support::direct_client();
    list_query_and_tags(&client).await;
}

#[tokio::test]
async fn http_list_query_and_tags() {
    let server = support::http_client().await;
    list_query_and_tags(&server.client).await;
}

async fn links_report_includes_backlinks(client: &dyn MemoryClient) {
    client
        .write_page(write_request(
            "projects/agentspace",
            "AgentSpace",
            &[],
            "the project",
        ))
        .await
        .unwrap_or_else(|error| panic!("write target: {error}"));
    client
        .write_page(write_request(
            "people/alice",
            "Alice",
            &[],
            "Related: [AgentSpace](../projects/agentspace.md)",
        ))
        .await
        .unwrap_or_else(|error| panic!("write referrer: {error}"));

    let report = client
        .links(path("projects/agentspace"), true)
        .await
        .unwrap_or_else(|error| panic!("links: {error}"));
    assert_eq!(report.backlinks.len(), 1);
    assert_eq!(report.backlinks[0].from, "people/alice");
}

#[tokio::test]
async fn direct_links_report_includes_backlinks() {
    let (client, _dir) = support::direct_client();
    links_report_includes_backlinks(&client).await;
}

#[tokio::test]
async fn http_links_report_includes_backlinks() {
    let server = support::http_client().await;
    links_report_includes_backlinks(&server.client).await;
}

async fn check_reports_clean_store(client: &dyn MemoryClient) {
    client
        .write_page(write_request("notes/plan", "Plan", &[], "no links here"))
        .await
        .unwrap_or_else(|error| panic!("write: {error}"));
    let report = client
        .check()
        .await
        .unwrap_or_else(|error| panic!("check: {error}"));
    assert!(report.is_clean(), "{report:?}");
}

#[tokio::test]
async fn direct_check_reports_clean_store() {
    let (client, _dir) = support::direct_client();
    check_reports_clean_store(&client).await;
}

#[tokio::test]
async fn http_check_reports_clean_store() {
    let server = support::http_client().await;
    check_reports_clean_store(&server.client).await;
}

async fn remove_page_then_read_reports_not_found(client: &dyn MemoryClient) {
    client
        .write_page(write_request("notes/scratch", "Scratch", &[], "temporary"))
        .await
        .unwrap_or_else(|error| panic!("write: {error}"));
    client
        .remove_page(RemovePageRequest {
            path: path("notes/scratch"),
            expected_revision: None,
        })
        .await
        .unwrap_or_else(|error| panic!("remove: {error}"));

    let error = client.read_page(path("notes/scratch")).await.map_or_else(
        |error| error,
        |page| panic!("expected not found, got {page:?}"),
    );
    assert!(matches!(error, MemoryError::NotFound { .. }), "{error:?}");
}

#[tokio::test]
async fn direct_remove_page_then_read_reports_not_found() {
    let (client, _dir) = support::direct_client();
    remove_page_then_read_reports_not_found(&client).await;
}

#[tokio::test]
async fn http_remove_page_then_read_reports_not_found() {
    let server = support::http_client().await;
    remove_page_then_read_reports_not_found(&server.client).await;
}

async fn run_allowed_command_exits_successfully(client: &dyn MemoryClient) {
    let stdout = VecSink::default();
    let stderr = VecSink::default();
    let outcome = client
        .run_command(
            vec!["pwd".to_owned()],
            RunLimits::default(),
            Box::new(stdout.clone()),
            Box::new(stderr),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("run: {error}"));
    assert_eq!(outcome, RunOutcome::Exited(0));
    assert!(!stdout.contents().is_empty());
}

#[tokio::test]
async fn direct_run_allowed_command_exits_successfully() {
    let (client, _dir) = support::direct_client();
    run_allowed_command_exits_successfully(&client).await;
}

#[tokio::test]
async fn http_run_allowed_command_exits_successfully() {
    let server = support::http_client().await;
    run_allowed_command_exits_successfully(&server.client).await;
}

/// `DirectMemoryClient` reports a disallowed command as
/// `Ok(RunOutcome::NotAllowed(..))`, matching `command_runner`'s existing
/// (pre-milestone-3) behavior; see the module-level note on why this is
/// not shared with the HTTP transport.
#[tokio::test]
async fn direct_run_rejects_disallowed_command_as_outcome() {
    let (client, _dir) = support::direct_client();
    let outcome = client
        .run_command(
            vec!["rm".to_owned(), "-rf".to_owned()],
            RunLimits::default(),
            Box::new(VecSink::default()),
            Box::new(VecSink::default()),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("run: {error}"));
    assert_eq!(outcome, RunOutcome::NotAllowed("rm".to_owned()));
}

/// The Axum adapter validates the executable allowlist before ever opening
/// a stream, so `HttpMemoryClient` observes this as an ordinary JSON error
/// response rather than a stream outcome; see the module-level note.
#[tokio::test]
async fn http_run_rejects_disallowed_command_as_error() {
    let server = support::http_client().await;
    let error = server
        .client
        .run_command(
            vec!["rm".to_owned(), "-rf".to_owned()],
            RunLimits::default(),
            Box::new(VecSink::default()),
            Box::new(VecSink::default()),
            Box::pin(std::future::pending()),
        )
        .await
        .map_or_else(
            |error| error,
            |outcome| panic!("expected error, got {outcome:?}"),
        );
    assert!(
        matches!(
            error,
            MemoryError::CommandNotAllowed { command } if command == "rm"
        ),
        "rejected command was not preserved"
    );
}
