use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

#[derive(Debug)]
pub enum AgentHostError {
    SessionNotFound { session_id: String },
    Validation { message: String },
    Runtime { message: String },
    Io { message: String },
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

    #[must_use]
    pub fn io(message: impl Into<String>) -> Self {
        Self::Io {
            message: message.into(),
        }
    }
}

impl Display for AgentHostError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionNotFound { session_id } => write!(formatter, "{session_id}"),
            Self::Validation { message } | Self::Runtime { message } | Self::Io { message } => {
                formatter.write_str(message)
            }
        }
    }
}

impl Error for AgentHostError {}

impl From<reqwest::Error> for AgentHostError {
    fn from(error: reqwest::Error) -> Self {
        Self::runtime(format!("kernel HTTP request failed: {error}"))
    }
}

impl From<std::io::Error> for AgentHostError {
    fn from(error: std::io::Error) -> Self {
        Self::io(format!("I/O error: {error}"))
    }
}

impl From<serde_json::Error> for AgentHostError {
    fn from(error: serde_json::Error) -> Self {
        Self::runtime(format!("JSON error: {error}"))
    }
}
