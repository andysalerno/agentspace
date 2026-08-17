use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use bollard::errors::Error as BollardError;

#[derive(Debug)]
pub enum AgentHostError {
    SessionNotFound { session_id: String },
    TerminalAttachmentNotFound { attachment_id: String },
    Validation { message: String },
    PayloadTooLarge { message: String },
    Conflict { message: String },
    UpstreamUnavailable { message: String },
    Runtime { message: String },
    Docker { source: BollardError },
    Http { source: reqwest::Error },
    Io { source: std::io::Error },
    Json { source: serde_json::Error },
}

impl AgentHostError {
    #[must_use]
    pub fn session_not_found(session_id: impl Into<String>) -> Self {
        Self::SessionNotFound {
            session_id: session_id.into(),
        }
    }

    #[must_use]
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self::PayloadTooLarge {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn terminal_attachment_not_found(attachment_id: impl Into<String>) -> Self {
        Self::TerminalAttachmentNotFound {
            attachment_id: attachment_id.into(),
        }
    }

    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn upstream_unavailable(message: impl Into<String>) -> Self {
        Self::UpstreamUnavailable {
            message: message.into(),
        }
    }
}

impl Display for AgentHostError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound { session_id } => write!(formatter, "{session_id}"),
            Self::TerminalAttachmentNotFound { attachment_id } => {
                write!(
                    formatter,
                    "terminal attachment {attachment_id:?} was not found"
                )
            }
            Self::Validation { message }
            | Self::PayloadTooLarge { message }
            | Self::Conflict { message }
            | Self::UpstreamUnavailable { message }
            | Self::Runtime { message } => formatter.write_str(message),
            Self::Docker { source } => write!(formatter, "Docker request failed: {source}"),
            Self::Http { source } => write!(formatter, "kernel HTTP request failed: {source}"),
            Self::Io { source } => write!(formatter, "I/O error: {source}"),
            Self::Json { source } => write!(formatter, "JSON error: {source}"),
        }
    }
}

impl Error for AgentHostError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Docker { source } => Some(source),
            Self::Http { source } => Some(source),
            Self::Io { source } => Some(source),
            Self::Json { source } => Some(source),
            Self::SessionNotFound { .. }
            | Self::TerminalAttachmentNotFound { .. }
            | Self::Validation { .. }
            | Self::PayloadTooLarge { .. }
            | Self::Conflict { .. }
            | Self::UpstreamUnavailable { .. }
            | Self::Runtime { .. } => None,
        }
    }
}

impl From<reqwest::Error> for AgentHostError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http { source: error }
    }
}

impl From<BollardError> for AgentHostError {
    fn from(error: BollardError) -> Self {
        Self::Docker { source: error }
    }
}

impl From<std::io::Error> for AgentHostError {
    fn from(error: std::io::Error) -> Self {
        Self::Io { source: error }
    }
}

impl From<serde_json::Error> for AgentHostError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json { source: error }
    }
}
