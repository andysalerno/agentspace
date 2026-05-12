use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

#[derive(Debug)]
pub enum AgentHostError {
    SessionNotFound { session_id: String },
    Validation { message: String },
    Runtime { message: String },
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
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime {
            message: message.into(),
        }
    }
}

impl Display for AgentHostError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound { session_id } => write!(formatter, "{session_id}"),
            Self::Validation { message } | Self::Runtime { message } => {
                formatter.write_str(message)
            }
            Self::Http { source } => write!(formatter, "kernel HTTP request failed: {source}"),
            Self::Io { source } => write!(formatter, "I/O error: {source}"),
            Self::Json { source } => write!(formatter, "JSON error: {source}"),
        }
    }
}

impl Error for AgentHostError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http { source } => Some(source),
            Self::Io { source } => Some(source),
            Self::Json { source } => Some(source),
            Self::SessionNotFound { .. } | Self::Validation { .. } | Self::Runtime { .. } => None,
        }
    }
}

impl From<reqwest::Error> for AgentHostError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http { source: error }
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
