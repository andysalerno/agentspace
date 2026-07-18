//! Focused tests for the `/v1/run` framed streaming protocol, exercised
//! specifically over the HTTP transport (the in-process behavior these
//! build on -- allowlist enforcement, timeout, and output-limit
//! termination -- is already covered by `command_runner`'s own unit tests).

mod support;

use std::time::Duration;

use memory_rs::{
    client::MemoryClient,
    command_runner::{RunLimits, RunOutcome},
    error::MemoryError,
    run_stream::RUN_CONTENT_TYPE,
};
use support::VecSink;

#[tokio::test]
async fn stdout_and_stderr_are_kept_separate() {
    let server = support::http_client().await;
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let existing = dir.path().join("exists.txt");
    std::fs::write(&existing, "hello-stdout\n")
        .unwrap_or_else(|error| panic!("write fixture: {error}"));

    let stdout = VecSink::default();
    let stderr = VecSink::default();
    let outcome = server
        .client
        .run_command(
            vec![
                "cat".to_owned(),
                existing.to_string_lossy().into_owned(),
                "/no/such/file-xyz".to_owned(),
            ],
            RunLimits::default(),
            Box::new(stdout.clone()),
            Box::new(stderr.clone()),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("run: {error}"));

    assert!(
        matches!(outcome, RunOutcome::Exited(code) if code != 0),
        "{outcome:?}"
    );
    assert_eq!(stdout.contents(), b"hello-stdout\n".to_vec());
    assert!(!stderr.contents().is_empty());
    // Never any cross-contamination between the two streams.
    assert!(!stdout.contents().windows(2).any(|w| w == b"No"));
}

#[cfg(unix)]
#[tokio::test]
async fn preserves_arbitrary_non_utf8_stdout_bytes() {
    let server = support::http_client().await;
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let payload: Vec<u8> = vec![0, 159, 146, 150, 255, 1, 2, 3, 0, 254];
    let fixture = dir.path().join("binary.bin");
    std::fs::write(&fixture, &payload).unwrap_or_else(|error| panic!("write fixture: {error}"));

    let stdout = VecSink::default();
    let outcome = server
        .client
        .run_command(
            vec!["cat".to_owned(), fixture.to_string_lossy().into_owned()],
            RunLimits::default(),
            Box::new(stdout.clone()),
            Box::new(VecSink::default()),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("run: {error}"));

    assert_eq!(outcome, RunOutcome::Exited(0));
    assert_eq!(stdout.contents(), payload);
}

#[tokio::test]
async fn exit_code_parity_between_direct_and_http() {
    let (direct, _dir) = support::direct_client();
    let http = support::http_client().await;

    let direct_outcome = direct
        .run_command(
            vec!["ls".to_owned(), "/no/such/directory-xyz".to_owned()],
            RunLimits::default(),
            Box::new(VecSink::default()),
            Box::new(VecSink::default()),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("direct run: {error}"));
    let http_outcome = http
        .client
        .run_command(
            vec!["ls".to_owned(), "/no/such/directory-xyz".to_owned()],
            RunLimits::default(),
            Box::new(VecSink::default()),
            Box::new(VecSink::default()),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("http run: {error}"));

    assert_eq!(direct_outcome, http_outcome);
    assert!(matches!(direct_outcome, RunOutcome::Exited(code) if code != 0));
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_terminates_a_long_running_command() {
    let server = support::http_client().await;
    let limits = RunLimits {
        timeout: Duration::from_millis(100),
        max_output_bytes: RunLimits::default().max_output_bytes,
    };
    let outcome = server
        .client
        .run_command(
            vec!["tail".to_owned(), "-f".to_owned(), "/dev/null".to_owned()],
            limits,
            Box::new(VecSink::default()),
            Box::new(VecSink::default()),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("run: {error}"));
    assert_eq!(outcome, RunOutcome::TimedOut);
}

#[cfg(unix)]
#[tokio::test]
async fn output_limit_terminates_a_noisy_command() {
    let server = support::http_client().await;
    let limits = RunLimits {
        timeout: Duration::from_secs(10),
        max_output_bytes: 16,
    };
    let outcome = server
        .client
        .run_command(
            vec!["cat".to_owned(), "/dev/zero".to_owned()],
            limits,
            Box::new(VecSink::default()),
            Box::new(VecSink::default()),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("run: {error}"));
    assert_eq!(outcome, RunOutcome::OutputLimitExceeded);
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_disconnects_and_kills_the_child() {
    let server = support::http_client().await;
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _send_result = cancel_tx.send(());
    });
    let cancel = Box::pin(async move {
        let _ = cancel_rx.await;
    });

    let outcome = server
        .client
        .run_command(
            vec!["tail".to_owned(), "-f".to_owned(), "/dev/null".to_owned()],
            RunLimits::default(),
            Box::new(VecSink::default()),
            Box::new(VecSink::default()),
            cancel,
        )
        .await
        .unwrap_or_else(|error| panic!("run: {error}"));
    assert_eq!(outcome, RunOutcome::Cancelled);

    // The server must remain usable for subsequent requests after a
    // cancelled stream; a stuck or leaked task would otherwise eventually
    // starve the server of resources.
    let followup = server
        .client
        .run_command(
            vec!["pwd".to_owned()],
            RunLimits::default(),
            Box::new(VecSink::default()),
            Box::new(VecSink::default()),
            Box::pin(std::future::pending()),
        )
        .await
        .unwrap_or_else(|error| panic!("followup run: {error}"));
    assert_eq!(followup, RunOutcome::Exited(0));
}

#[tokio::test]
async fn unavailable_service_reports_unavailable_error() {
    let client = memory_rs::http_client::HttpMemoryClient::new("http://127.0.0.1:0");
    let error = client
        .run_command(
            vec!["pwd".to_owned()],
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
        matches!(error, MemoryError::Unavailable { .. }),
        "{error:?}"
    );
}

/// A server that claims the run content type but never sends a terminal
/// frame must be reported as malformed, never treated as a silently
/// successful (or silently empty) run.
#[tokio::test]
async fn malformed_response_without_terminal_frame_is_reported() {
    use axum::{
        Router,
        response::{IntoResponse, Response},
        routing::post,
    };

    async fn fake_run() -> Response {
        // A well-formed stdout chunk frame, but the stream ends here: no
        // terminal frame ever arrives.
        let frame =
            memory_rs::run_stream::encode_chunk(memory_rs::run_stream::TAG_STDOUT, b"partial");
        (
            [(axum::http::header::CONTENT_TYPE, RUN_CONTENT_TYPE)],
            frame,
        )
            .into_response()
    }

    let app = Router::new().route("/v1/run", post(fake_run));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("bind: {error}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("local_addr: {error}"));
    let server_task = tokio::spawn(async move {
        let _serve_result = axum::serve(listener, app).await;
    });

    let client = memory_rs::http_client::HttpMemoryClient::new(format!("http://{addr}"));
    let error = client
        .run_command(
            vec!["pwd".to_owned()],
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
        matches!(error, MemoryError::MalformedResponse { .. }),
        "{error:?}"
    );

    server_task.abort();
}
