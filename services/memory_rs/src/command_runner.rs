//! Executes a fixed, read-oriented allowlist of commands directly.
//!
//! Commands run directly (never through a shell) with separate
//! stdout/stderr streaming, exit-code preservation, cancellation, and
//! configurable timeout/output-byte limits.
//!
//! This is a convenience boundary, not a security boundary: only the
//! executable name is checked against the allowlist, and all remaining
//! arguments are passed through unchanged.

use std::{
    fmt::{self, Display, Formatter},
    future::Future,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _},
    process::Command,
    sync::Notify,
};

use crate::error::MISSING_COMMAND;

/// The fixed set of executables `memory run` is allowed to invoke.
pub const ALLOWED_COMMANDS: &[&str] = &["rg", "ls", "cat", "head", "tail", "wc", "stat", "pwd"];

/// Returns whether `command` is in the fixed read-oriented allowlist.
#[must_use]
pub fn is_allowed(command: &str) -> bool {
    ALLOWED_COMMANDS.contains(&command)
}

/// Execution-time and output-byte limits applied to every `memory run`
/// invocation.
#[derive(Clone, Copy, Debug)]
pub struct RunLimits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_output_bytes: 10 * 1024 * 1024,
        }
    }
}

/// How a `memory run` invocation ended, distinguishing the invoked
/// command's own exit code from every other termination reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// The child process exited on its own with this exit code.
    Exited(i32),
    /// The configured timeout elapsed before the child exited.
    TimedOut,
    /// The configured output-byte limit was reached; the child was killed.
    OutputLimitExceeded,
    /// The caller-supplied cancellation future resolved before the child
    /// exited; the child was killed.
    Cancelled,
    /// The first argument is not in [`ALLOWED_COMMANDS`].
    NotAllowed(String),
    /// The child process could not be spawned.
    LaunchFailed(String),
}

impl Display for RunOutcome {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited(code) => write!(formatter, "exited with code {code}"),
            Self::TimedOut => formatter.write_str("timed out"),
            Self::OutputLimitExceeded => formatter.write_str("output limit exceeded"),
            Self::Cancelled => formatter.write_str("cancelled"),
            Self::NotAllowed(command) => write!(formatter, "command {command:?} is not allowed"),
            Self::LaunchFailed(message) => write!(formatter, "failed to launch: {message}"),
        }
    }
}

/// Validates `argv[0]` against [`ALLOWED_COMMANDS`] and spawns it.
///
/// Spawns directly (never through a shell) with `cwd` as its working
/// directory and a minimal environment (only `PATH`, so allowlisted tools
/// can still be located).
///
/// Kept separate from [`drive`] so a caller such as the Axum `/v1/run`
/// adapter can distinguish a rejected or unlaunchable command — reported
/// before any response streaming begins — from every outcome that can only
/// be known once the child is running.
///
/// # Errors
///
/// Returns [`RunOutcome::NotAllowed`] if `argv` is empty or its first
/// element is not in [`ALLOWED_COMMANDS`], or [`RunOutcome::LaunchFailed`]
/// if the process could not be spawned.
pub fn spawn(cwd: &Path, argv: &[String]) -> Result<tokio::process::Child, RunOutcome> {
    let Some(program) = argv.first() else {
        return Err(RunOutcome::NotAllowed(MISSING_COMMAND.to_owned()));
    };
    if !is_allowed(program) {
        return Err(RunOutcome::NotAllowed(program.clone()));
    }

    let mut command = Command::new(program);
    command.args(&argv[1..]);
    command.current_dir(cwd);
    command.kill_on_drop(true);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.env_clear();
    if let Ok(path_var) = std::env::var("PATH") {
        command.env("PATH", path_var);
    }

    command
        .spawn()
        .map_err(|error| RunOutcome::LaunchFailed(error.to_string()))
}

/// Runs `argv` directly (never through a shell).
///
/// Uses `cwd` as its working directory, streaming stdout/stderr
/// independently into the given sinks as bytes arrive, honoring `limits`,
/// and resolving early if `cancel` completes first.
///
/// Only `argv[0]` is validated against [`ALLOWED_COMMANDS`]; every remaining
/// argument is passed through unchanged.
pub async fn run<Out, ErrOut, Cancel>(
    cwd: &Path,
    argv: &[String],
    limits: RunLimits,
    stdout_sink: Out,
    stderr_sink: ErrOut,
    cancel: Cancel,
) -> RunOutcome
where
    Out: AsyncWrite + Unpin + Send + 'static,
    ErrOut: AsyncWrite + Unpin + Send + 'static,
    Cancel: Future<Output = ()> + Send + 'static,
{
    let child = match spawn(cwd, argv) {
        Ok(child) => child,
        Err(outcome) => return outcome,
    };
    drive(child, limits, stdout_sink, stderr_sink, cancel).await
}

/// Drives an already-spawned child to completion, streaming stdout/stderr
/// independently into the given sinks as bytes arrive, honoring `limits`,
/// and resolving early if `cancel` completes first.
pub async fn drive<Out, ErrOut, Cancel>(
    mut child: tokio::process::Child,
    limits: RunLimits,
    stdout_sink: Out,
    stderr_sink: ErrOut,
    cancel: Cancel,
) -> RunOutcome
where
    Out: AsyncWrite + Unpin + Send + 'static,
    ErrOut: AsyncWrite + Unpin + Send + 'static,
    Cancel: Future<Output = ()> + Send + 'static,
{
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();

    let total_bytes = Arc::new(AtomicUsize::new(0));
    let limit_hit = Arc::new(AtomicBool::new(false));
    let limit_notify = Arc::new(Notify::new());

    let stdout_task = tokio::spawn(copy_capped(
        child_stdout,
        stdout_sink,
        Arc::clone(&total_bytes),
        Arc::clone(&limit_hit),
        Arc::clone(&limit_notify),
        limits.max_output_bytes,
    ));
    let stderr_task = tokio::spawn(copy_capped(
        child_stderr,
        stderr_sink,
        Arc::clone(&total_bytes),
        Arc::clone(&limit_hit),
        Arc::clone(&limit_notify),
        limits.max_output_bytes,
    ));

    tokio::pin!(cancel);
    let sleep = tokio::time::sleep(limits.timeout);
    tokio::pin!(sleep);

    let outcome = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => RunOutcome::Exited(status.code().unwrap_or(-1)),
            Err(error) => RunOutcome::LaunchFailed(error.to_string()),
        },
        () = &mut sleep => {
            let _kill_result = child.start_kill();
            let _wait_result = child.wait().await;
            RunOutcome::TimedOut
        }
        () = &mut cancel => {
            let _kill_result = child.start_kill();
            let _wait_result = child.wait().await;
            RunOutcome::Cancelled
        }
        () = limit_notify.notified() => {
            let _kill_result = child.start_kill();
            let _wait_result = child.wait().await;
            RunOutcome::OutputLimitExceeded
        }
    };

    let _stdout_join_result = stdout_task.await;
    let _stderr_join_result = stderr_task.await;

    if limit_hit.load(Ordering::Relaxed) {
        let _kill_result = child.start_kill();
        return RunOutcome::OutputLimitExceeded;
    }

    outcome
}

async fn copy_capped<R, W>(
    reader: Option<R>,
    mut writer: W,
    total: Arc<AtomicUsize>,
    limit_hit: Arc<AtomicBool>,
    limit_notify: Arc<Notify>,
    max_bytes: usize,
) where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let Some(mut reader) = reader else {
        return;
    };
    let mut buffer = [0_u8; 8192];
    loop {
        if limit_hit.load(Ordering::Relaxed) {
            break;
        }
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };

        let previous_total = total.fetch_add(read, Ordering::Relaxed);
        let allowed = max_bytes.saturating_sub(previous_total.min(max_bytes));
        let to_write = read.min(allowed);
        if to_write > 0 {
            let _write_result = writer.write_all(&buffer[..to_write]).await;
        }
        if previous_total + read >= max_bytes {
            limit_hit.store(true, Ordering::Relaxed);
            limit_notify.notify_one();
            break;
        }
    }
    let _flush_result = writer.flush().await;
}

#[cfg(test)]
mod tests {
    use std::{
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll},
    };

    use tokio::io::AsyncWrite;

    use super::{RunLimits, RunOutcome, run};

    #[derive(Clone, Default)]
    struct VecSink(Arc<Mutex<Vec<u8>>>);

    impl VecSink {
        fn contents(&self) -> Vec<u8> {
            self.0
                .lock()
                .unwrap_or_else(|error| panic!("lock: {error}"))
                .clone()
        }
    }

    impl AsyncWrite for VecSink {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.0
                .lock()
                .unwrap_or_else(|error| panic!("lock: {error}"))
                .extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn rejects_commands_outside_allowlist() {
        let stdout = VecSink::default();
        let stderr = VecSink::default();
        let outcome = run(
            std::path::Path::new("."),
            &["rm".to_owned(), "-rf".to_owned()],
            RunLimits::default(),
            stdout,
            stderr,
            std::future::pending(),
        )
        .await;
        assert_eq!(outcome, RunOutcome::NotAllowed("rm".to_owned()));
    }

    #[tokio::test]
    async fn streams_stdout_and_preserves_exit_code() {
        let stdout = VecSink::default();
        let stderr = VecSink::default();
        let outcome = run(
            std::path::Path::new("."),
            &["pwd".to_owned()],
            RunLimits::default(),
            stdout.clone(),
            stderr,
            std::future::pending(),
        )
        .await;
        assert_eq!(outcome, RunOutcome::Exited(0));
        assert!(!stdout.contents().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn enforces_output_byte_limit() {
        let stdout = VecSink::default();
        let stderr = VecSink::default();
        let limits = RunLimits {
            timeout: std::time::Duration::from_secs(5),
            max_output_bytes: 4,
        };
        let outcome = run(
            std::path::Path::new("."),
            &["cat".to_owned(), "/dev/zero".to_owned()],
            limits,
            stdout,
            stderr,
            std::future::pending(),
        )
        .await;
        assert_eq!(outcome, RunOutcome::OutputLimitExceeded);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_limit_kills_an_idle_child_promptly() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let input = directory.path().join("large.txt");
        std::fs::write(&input, vec![b'x'; 32 * 1024])
            .unwrap_or_else(|error| panic!("write input: {error}"));
        let limits = RunLimits {
            timeout: std::time::Duration::from_secs(10),
            max_output_bytes: 16,
        };
        let started = std::time::Instant::now();
        let outcome = run(
            directory.path(),
            &[
                "tail".to_owned(),
                "-f".to_owned(),
                input.to_string_lossy().into_owned(),
            ],
            limits,
            VecSink::default(),
            VecSink::default(),
            std::future::pending(),
        )
        .await;

        assert_eq!(outcome, RunOutcome::OutputLimitExceeded);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn enforces_timeout() {
        let stdout = VecSink::default();
        let stderr = VecSink::default();
        let limits = RunLimits {
            timeout: std::time::Duration::from_millis(50),
            max_output_bytes: RunLimits::default().max_output_bytes,
        };
        let outcome = run(
            std::path::Path::new("."),
            &["tail".to_owned(), "-f".to_owned(), "/dev/null".to_owned()],
            limits,
            stdout,
            stderr,
            std::future::pending(),
        )
        .await;
        assert_eq!(outcome, RunOutcome::TimedOut);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_stops_the_child() {
        let stdout = VecSink::default();
        let stderr = VecSink::default();
        let outcome = run(
            std::path::Path::new("."),
            &["tail".to_owned(), "-f".to_owned(), "/dev/null".to_owned()],
            RunLimits::default(),
            stdout,
            stderr,
            async {},
        )
        .await;
        assert_eq!(outcome, RunOutcome::Cancelled);
    }

    #[tokio::test]
    async fn launch_failure_is_reported() {
        let stdout = VecSink::default();
        let stderr = VecSink::default();
        // `wc` is allowlisted but a nonexistent path argument is passed
        // through unchanged; the process should still launch and exit
        // non-zero rather than failing to launch.
        let outcome = run(
            std::path::Path::new("."),
            &["wc".to_owned(), "/no/such/file".to_owned()],
            RunLimits::default(),
            stdout,
            stderr,
            std::future::pending(),
        )
        .await;
        assert!(matches!(outcome, RunOutcome::Exited(code) if code != 0));
    }
}
