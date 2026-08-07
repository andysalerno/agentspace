use std::{
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::AgentHostError;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessName {
    ClaudeCode,
    Echo,
    CopilotCli,
    Codex,
    Opencode,
    #[default]
    Acp,
}

impl HarnessName {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Echo => "echo",
            Self::CopilotCli => "copilot-cli",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Acp => "acp",
        }
    }
}

impl Display for HarnessName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for HarnessName {
    type Err = AgentHostError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "claude-code" => Ok(Self::ClaudeCode),
            "echo" => Ok(Self::Echo),
            "copilot-cli" => Ok(Self::CopilotCli),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::Opencode),
            "acp" => Ok(Self::Acp),
            _ => Err(AgentHostError::validation(format!(
                "unknown harness {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KernelStatus {
    #[default]
    Idle,
    Busy,
    Error,
    Done,
}

impl KernelStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Error => "error",
            Self::Done => "done",
        }
    }
}

impl Display for KernelStatus {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for KernelStatus {
    type Err = AgentHostError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "idle" => Ok(Self::Idle),
            "busy" => Ok(Self::Busy),
            "error" => Ok(Self::Error),
            "done" => Ok(Self::Done),
            _ => Err(AgentHostError::validation(format!(
                "unknown kernel status {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KernelEventType {
    SessionStart,
    SessionStatus,
    SessionUpdate,
    SessionPromptResult,
    SessionError,
    SessionEnd,
    Status,
    TextDelta,
    ReasoningDelta,
    ToolCall,
    ToolResult,
    Error,
    LegacySessionStart,
    LegacySessionEnd,
    Unknown(String),
}

impl KernelEventType {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "session/start",
            Self::SessionStatus => "session/status",
            Self::SessionUpdate => "session/update",
            Self::SessionPromptResult => "session/prompt/result",
            Self::SessionError => "session/error",
            Self::SessionEnd => "session/end",
            Self::Status => "status",
            Self::TextDelta => "text_delta",
            Self::ReasoningDelta => "reasoning_delta",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::Error => "error",
            Self::LegacySessionStart => "session_start",
            Self::LegacySessionEnd => "session_end",
            Self::Unknown(event_type) => event_type,
        }
    }
}

impl Display for KernelEventType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for KernelEventType {
    fn from(event_type: &str) -> Self {
        match event_type {
            "session/start" => Self::SessionStart,
            "session/status" => Self::SessionStatus,
            "session/update" => Self::SessionUpdate,
            "session/prompt/result" => Self::SessionPromptResult,
            "session/error" => Self::SessionError,
            "session/end" => Self::SessionEnd,
            "status" => Self::Status,
            "text_delta" => Self::TextDelta,
            "reasoning_delta" => Self::ReasoningDelta,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            "error" => Self::Error,
            "session_start" => Self::LegacySessionStart,
            "session_end" => Self::LegacySessionEnd,
            unknown => Self::Unknown(unknown.to_owned()),
        }
    }
}

impl From<String> for KernelEventType {
    fn from(event_type: String) -> Self {
        match event_type.as_str() {
            "session/start" => Self::SessionStart,
            "session/status" => Self::SessionStatus,
            "session/update" => Self::SessionUpdate,
            "session/prompt/result" => Self::SessionPromptResult,
            "session/error" => Self::SessionError,
            "session/end" => Self::SessionEnd,
            "status" => Self::Status,
            "text_delta" => Self::TextDelta,
            "reasoning_delta" => Self::ReasoningDelta,
            "tool_call" => Self::ToolCall,
            "tool_result" => Self::ToolResult,
            "error" => Self::Error,
            "session_start" => Self::LegacySessionStart,
            "session_end" => Self::LegacySessionEnd,
            _ => Self::Unknown(event_type),
        }
    }
}

impl Serialize for KernelEventType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for KernelEventType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let event_type = String::deserialize(deserializer)?;
        Ok(Self::from(event_type))
    }
}

fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct KernelEvent {
    #[serde(rename = "type")]
    pub event_type: KernelEventType,
    #[serde(default = "utc_now")]
    pub ts: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<KernelStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl KernelEvent {
    #[must_use]
    pub fn new(event_type: impl Into<KernelEventType>) -> Self {
        Self {
            event_type: event_type.into(),
            ts: utc_now(),
            session_id: None,
            kernel: None,
            status: None,
            method: None,
            params: None,
            update: None,
            result: None,
            error: None,
            content: None,
            tool: None,
            input: None,
            output: None,
            message: None,
        }
    }

    pub fn to_jsonl(&self) -> Result<String, AgentHostError> {
        serde_json::to_string(self).map_err(AgentHostError::from)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceMount {
    pub workspace_id: String,
    #[serde(default)]
    pub mode: WorkspaceMountMode,
    #[serde(default)]
    pub volume_name: Option<String>,
}

impl WorkspaceMount {
    #[must_use]
    pub fn new(workspace_id: impl Into<String>, mode: WorkspaceMountMode) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            mode,
            volume_name: None,
        }
    }

    #[must_use]
    pub fn effective_volume_name(&self) -> String {
        self.volume_name
            .clone()
            .unwrap_or_else(|| format!("agentspace-workspace-{}", self.workspace_id))
    }

    #[must_use]
    pub fn mount_path(&self) -> String {
        format!("/workspace/{}", self.workspace_id)
    }

    #[must_use]
    pub fn summary(&self) -> WorkspaceMountSummary {
        WorkspaceMountSummary {
            workspace_id: self.workspace_id.clone(),
            mode: self.mode,
            mount_path: self.mount_path(),
            volume_name: self.effective_volume_name(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum WorkspaceMountMode {
    #[serde(rename = "ro")]
    ReadOnly,
    #[default]
    #[serde(rename = "rw")]
    ReadWrite,
}

impl WorkspaceMountMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "ro",
            Self::ReadWrite => "rw",
        }
    }
}

impl Display for WorkspaceMountMode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceMountSummary {
    pub workspace_id: String,
    pub mode: WorkspaceMountMode,
    pub mount_path: String,
    pub volume_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DockerKernelSession {
    pub container_name: String,
    pub session_workspace_volume_name: String,
    pub base_url: String,
    #[serde(skip)]
    pub terminal_token: String,
    pub vscode_url: Option<String>,
    pub free_port_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value")]
pub enum KernelRuntimeSession {
    Docker(DockerKernelSession),
    Opaque(String),
}

impl KernelRuntimeSession {
    #[must_use]
    pub fn opaque(value: impl Into<String>) -> Self {
        Self::Opaque(value.into())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DockerStatsSummary {
    pub cpu_percent: Option<f64>,
    pub memory_usage_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
    pub memory_percent: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeSessionSummary {
    pub status: Option<KernelStatus>,
    pub resume_token: Option<String>,
    pub vscode_url: Option<String>,
    pub free_port_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub harness: HarnessName,
    pub status: KernelStatus,
    pub turns: usize,
    pub resume_token: Option<String>,
    pub additional_paths: Vec<String>,
    pub workspace_mounts: Vec<WorkspaceMountSummary>,
    pub container_name: Option<String>,
    pub vscode_url: Option<String>,
    pub free_port_url: Option<String>,
    pub stats: Option<DockerStatsSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServiceSummary {
    pub status: &'static str,
    pub detail: &'static str,
}

impl ServiceSummary {
    #[must_use]
    pub const fn placeholder(detail: &'static str) -> Self {
        Self {
            status: "placeholder",
            detail,
        }
    }

    #[must_use]
    pub const fn ready(detail: &'static str) -> Self {
        Self {
            status: "ready",
            detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{KernelEvent, KernelEventType, KernelStatus, WorkspaceMount, WorkspaceMountMode};

    #[test]
    fn kernel_event_serialization_omits_absent_fields() {
        let mut event = KernelEvent::new(KernelEventType::SessionStatus);
        event.status = Some(KernelStatus::Busy);

        let payload = serde_json::to_value(&event)
            .unwrap_or_else(|error| panic!("failed to serialize event: {error}"));

        assert_eq!(payload["type"], "session/status");
        assert_eq!(payload["status"], "busy");
        assert!(payload.get("content").is_none());
    }

    #[test]
    fn workspace_mount_summary_uses_python_shape() {
        let mount = WorkspaceMount {
            workspace_id: "todo-list-code".to_owned(),
            mode: WorkspaceMountMode::ReadOnly,
            volume_name: None,
        };

        let payload = serde_json::to_value(mount.summary())
            .unwrap_or_else(|error| panic!("failed to serialize mount: {error}"));

        assert_eq!(
            payload,
            json!({
                "workspace_id": "todo-list-code",
                "mode": "ro",
                "mount_path": "/workspace/todo-list-code",
                "volume_name": "agentspace-workspace-todo-list-code"
            })
        );
    }
}
