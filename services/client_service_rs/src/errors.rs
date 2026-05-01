use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    InvalidAgentId { value: String },
    InvalidConnectionId { value: String },
    InvalidGatewayId { value: String },
    InvalidSkillId { value: String },
    InvalidWorkspaceId { value: String },
    InvalidWorkspaceStatus { value: String },
    InvalidConnectionApiFlavor { value: String },
    InvalidHarnessName { value: String },
    InvalidGatewayType { value: String },
}

impl Display for ValidationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAgentId { value } => write!(
                formatter,
                "agent_id must use lowercase letters and single dashes only, got {value:?}"
            ),
            Self::InvalidConnectionId { value } => write!(
                formatter,
                "connection_id must use lowercase letters and single dashes only, got {value:?}"
            ),
            Self::InvalidGatewayId { value } => write!(
                formatter,
                "gateway_id must use lowercase letters and single dashes only, got {value:?}"
            ),
            Self::InvalidSkillId { value } => write!(
                formatter,
                "skill_id must use lowercase letters, digits, and single dashes only, got {value:?}"
            ),
            Self::InvalidWorkspaceId { value } => write!(
                formatter,
                "workspace_id must use lowercase letters, digits, and single dashes only, got {value:?}"
            ),
            Self::InvalidWorkspaceStatus { value } => {
                write!(
                    formatter,
                    "workspace status must be creating, ready, or failed, got {value:?}"
                )
            }
            Self::InvalidConnectionApiFlavor { value } => write!(
                formatter,
                "connection api_flavor must be chat_completions or responses, got {value:?}"
            ),
            Self::InvalidHarnessName { value } => {
                write!(formatter, "unsupported harness name {value:?}")
            }
            Self::InvalidGatewayType { value } => {
                write!(formatter, "unsupported gateway type {value:?}")
            }
        }
    }
}

impl Error for ValidationError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    AgentAlreadyExists { agent_id: String },
    AgentNotFound { agent_id: String },
    ConnectionAlreadyExists { connection_id: String },
    ConnectionNotFound { connection_id: String },
    GatewayAlreadyExists { gateway_id: String },
    GatewayNotFound { gateway_id: String },
    WorkspaceAlreadyExists { workspace_id: String },
    WorkspaceNotFound { workspace_id: String },
    SessionAlreadyExists { session_id: String },
    SessionNotFound { session_id: String },
    LockPoisoned { store: &'static str },
    Persistence { store: &'static str, detail: String },
}

impl Display for StoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentAlreadyExists { agent_id } => {
                write!(formatter, "agent {agent_id:?} already exists")
            }
            Self::AgentNotFound { agent_id } => write!(formatter, "agent {agent_id:?} not found"),
            Self::ConnectionAlreadyExists { connection_id } => {
                write!(formatter, "connection {connection_id:?} already exists")
            }
            Self::ConnectionNotFound { connection_id } => {
                write!(formatter, "connection {connection_id:?} not found")
            }
            Self::GatewayAlreadyExists { gateway_id } => {
                write!(formatter, "gateway {gateway_id:?} already exists")
            }
            Self::GatewayNotFound { gateway_id } => {
                write!(formatter, "gateway {gateway_id:?} not found")
            }
            Self::WorkspaceAlreadyExists { workspace_id } => {
                write!(formatter, "workspace {workspace_id:?} already exists")
            }
            Self::WorkspaceNotFound { workspace_id } => {
                write!(formatter, "workspace {workspace_id:?} not found")
            }
            Self::SessionAlreadyExists { session_id } => {
                write!(formatter, "session {session_id:?} already exists")
            }
            Self::SessionNotFound { session_id } => {
                write!(formatter, "session {session_id:?} not found")
            }
            Self::LockPoisoned { store } => write!(formatter, "{store} store lock is poisoned"),
            Self::Persistence { store, detail } => {
                write!(formatter, "{store} store persistence error: {detail}")
            }
        }
    }
}

impl Error for StoreError {}
