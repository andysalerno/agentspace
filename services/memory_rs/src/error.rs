use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

/// Stable rejected-command value used when a run request omits `argv[0]`.
pub const MISSING_COMMAND: &str = "<missing>";

/// Domain-level errors shared by every transport and command.
///
/// Both `DirectMemoryClient` and a future `HttpMemoryClient` map to and from
/// this single error type so validation, conflict, and not-found semantics
/// stay identical regardless of transport.
#[derive(Debug)]
pub enum MemoryError {
    InvalidPath {
        path: String,
        reason: String,
    },
    NotFound {
        path: String,
    },
    AlreadyExists {
        path: String,
    },
    Conflict {
        path: String,
        expected: Option<String>,
        actual: String,
    },
    InvalidFrontmatter {
        path: String,
        reason: String,
    },
    TooLarge {
        what: &'static str,
        limit: usize,
    },
    CommandNotAllowed {
        command: String,
    },
    RunTimedOut,
    RunOutputLimitExceeded,
    RunCancelled,
    RunLaunchFailed {
        message: String,
    },
    Lock {
        message: String,
    },
    NotImplemented {
        feature: String,
    },
    /// The configured remote transport could not complete the request:
    /// connection refused, DNS failure, or a bounded connect/request
    /// timeout elapsed. Never used by the local/direct transport.
    Unavailable {
        message: String,
    },
    /// A remote transport received a response that could not be
    /// interpreted as a valid reply to the request that was sent: an
    /// unexpected content type, invalid JSON, or a `/v1/run` byte stream
    /// that ended without a terminal frame.
    MalformedResponse {
        message: String,
    },
    /// A failure that does not fit any other category, surfaced by either
    /// transport.
    Internal {
        message: String,
    },
    Io {
        source: std::io::Error,
    },
    Yaml {
        source: serde_yaml_ng::Error,
    },
}

impl MemoryError {
    #[must_use]
    pub fn invalid_path(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidPath {
            path: path.into(),
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn not_found(path: impl Into<String>) -> Self {
        Self::NotFound { path: path.into() }
    }

    #[must_use]
    pub fn already_exists(path: impl Into<String>) -> Self {
        Self::AlreadyExists { path: path.into() }
    }

    #[must_use]
    pub fn conflict(path: impl Into<String>, expected: Option<String>, actual: String) -> Self {
        Self::Conflict {
            path: path.into(),
            expected,
            actual,
        }
    }

    #[must_use]
    pub fn invalid_frontmatter(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidFrontmatter {
            path: path.into(),
            reason: reason.into(),
        }
    }

    #[must_use]
    pub const fn too_large(what: &'static str, limit: usize) -> Self {
        Self::TooLarge { what, limit }
    }

    #[must_use]
    pub fn command_not_allowed(command: impl Into<String>) -> Self {
        Self::CommandNotAllowed {
            command: command.into(),
        }
    }

    #[must_use]
    pub fn run_launch_failed(message: impl Into<String>) -> Self {
        Self::RunLaunchFailed {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn lock(message: impl Into<String>) -> Self {
        Self::Lock {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn not_implemented(feature: impl Into<String>) -> Self {
        Self::NotImplemented {
            feature: feature.into(),
        }
    }

    #[must_use]
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn malformed_response(message: impl Into<String>) -> Self {
        Self::MalformedResponse {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    /// A stable machine-readable kind, used for `--json` output and process
    /// exit code mapping.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::InvalidPath { .. } => "invalid_path",
            Self::NotFound { .. } => "not_found",
            Self::AlreadyExists { .. } => "already_exists",
            Self::Conflict { .. } => "conflict",
            Self::InvalidFrontmatter { .. } => "invalid_frontmatter",
            Self::TooLarge { .. } => "too_large",
            Self::CommandNotAllowed { .. } => "command_not_allowed",
            Self::RunTimedOut => "run_timed_out",
            Self::RunOutputLimitExceeded => "run_output_limit_exceeded",
            Self::RunCancelled => "run_cancelled",
            Self::RunLaunchFailed { .. } => "run_launch_failed",
            Self::Lock { .. } => "lock",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Unavailable { .. } => "unavailable",
            Self::MalformedResponse { .. } => "malformed_response",
            Self::Internal { .. } => "internal",
            Self::Io { .. } => "io",
            Self::Yaml { .. } => "yaml",
        }
    }
}

impl Display for MemoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath { path, reason } => {
                write!(formatter, "invalid page path {path:?}: {reason}")
            }
            Self::NotFound { path } => write!(formatter, "page not found: {path}"),
            Self::AlreadyExists { path } => write!(formatter, "page already exists: {path}"),
            Self::Conflict {
                path,
                expected,
                actual,
            } => match expected {
                Some(expected) => write!(
                    formatter,
                    "revision conflict for {path}: expected {expected}, found {actual}"
                ),
                None => write!(
                    formatter,
                    "revision conflict for {path}: page already exists with revision {actual}"
                ),
            },
            Self::InvalidFrontmatter { path, reason } => {
                write!(formatter, "invalid frontmatter in {path}: {reason}")
            }
            Self::TooLarge { what, limit } => {
                write!(
                    formatter,
                    "{what} exceeds the configured limit of {limit} bytes"
                )
            }
            Self::CommandNotAllowed { command } => {
                if command == MISSING_COMMAND {
                    formatter.write_str("command is required")
                } else {
                    write!(formatter, "command {command:?} is not in the allowlist")
                }
            }
            Self::RunTimedOut => formatter.write_str("command timed out"),
            Self::RunOutputLimitExceeded => {
                formatter.write_str("command output exceeded the configured limit")
            }
            Self::RunCancelled => formatter.write_str("command was cancelled"),
            Self::RunLaunchFailed { message } => {
                write!(formatter, "command failed to launch: {message}")
            }
            Self::Lock { message } => write!(formatter, "failed to lock memory store: {message}"),
            Self::NotImplemented { feature } => {
                write!(formatter, "{feature} is not implemented yet")
            }
            Self::Unavailable { message } => {
                write!(formatter, "memory service is unavailable: {message}")
            }
            Self::MalformedResponse { message } => {
                write!(
                    formatter,
                    "memory service returned a malformed response: {message}"
                )
            }
            Self::Internal { message } => write!(formatter, "internal error: {message}"),
            Self::Io { source } => write!(formatter, "I/O error: {source}"),
            Self::Yaml { source } => write!(formatter, "YAML error: {source}"),
        }
    }
}

impl Error for MemoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source } => Some(source),
            Self::Yaml { source } => Some(source),
            Self::InvalidPath { .. }
            | Self::NotFound { .. }
            | Self::AlreadyExists { .. }
            | Self::Conflict { .. }
            | Self::InvalidFrontmatter { .. }
            | Self::TooLarge { .. }
            | Self::CommandNotAllowed { .. }
            | Self::RunTimedOut
            | Self::RunOutputLimitExceeded
            | Self::RunCancelled
            | Self::RunLaunchFailed { .. }
            | Self::Lock { .. }
            | Self::NotImplemented { .. }
            | Self::Unavailable { .. }
            | Self::MalformedResponse { .. }
            | Self::Internal { .. } => None,
        }
    }
}

impl From<std::io::Error> for MemoryError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

impl From<serde_yaml_ng::Error> for MemoryError {
    fn from(source: serde_yaml_ng::Error) -> Self {
        Self::Yaml { source }
    }
}
