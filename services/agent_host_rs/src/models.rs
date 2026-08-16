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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InteractionMode {
    #[default]
    Chat,
    Cli,
}

impl InteractionMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Cli => "cli",
        }
    }
}

impl Display for InteractionMode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
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
    pub session_id: String,
    pub container_name: String,
    pub session_workspace_volume_name: String,
    pub session_telemetry_volume_name: Option<String>,
    pub base_url: String,
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryState {
    Starting,
    Live,
    Stale,
    #[default]
    Unavailable,
    Degraded,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryContentMode {
    #[default]
    Metadata,
    Content,
    PolicyConflict,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheReportingState {
    Reported,
    #[default]
    Unreported,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenAccountingConvention {
    Inclusive,
    Additive,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSignalState {
    Healthy,
    CacheResetSuspected,
    ExpectedBoundary,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSignalConfidence {
    Low,
    Medium,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSignalReason {
    ReuseCollapsed,
    ContextDiscontinuity,
    CompactionOrTruncation,
    ModelChanged,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryWarningCode {
    CheckpointCorrupt,
    CheckpointNewerVersion,
    ContentPolicyConflict,
    DuplicateConflict,
    FieldTruncated,
    FileLimitExceeded,
    InvalidUsageShape,
    LineTooLong,
    MalformedRecord,
    PartialRecordDiscarded,
    SizeLimitExceeded,
    SourceFileChanged,
    SpanLimitExceeded,
    UnknownRecord,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct UsageBreakdown {
    pub raw_input_tokens: Option<u64>,
    pub effective_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub other_input_tokens: Option<u64>,
    pub fresh_input_tokens: Option<u64>,
    pub cache_reuse_percent: Option<f64>,
    pub nano_aiu: Option<u64>,
    pub opaque_cost: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ModelCallSummary {
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub duration_ms: Option<u64>,
    pub model: Option<String>,
    pub requested_model: Option<String>,
    pub provider: Option<String>,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub is_subagent: bool,
    pub cache_reporting: CacheReportingState,
    pub token_accounting_convention: TokenAccountingConvention,
    pub usage: UsageBreakdown,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ActivityCounts {
    pub interactions: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
    pub subagent_invocations: u64,
    pub subagent_model_calls: u64,
    pub errors: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ReportingCoverage {
    pub model_calls: u64,
    pub cache_reported_calls: u64,
    pub convention_resolved_calls: u64,
    pub effective_input_covered_calls: u64,
    pub context_reported: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub limit: Option<u64>,
    pub message_count: Option<u64>,
    pub observed_at: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SubagentBreakdown {
    pub invocations: u64,
    pub model_calls: u64,
    pub effective_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct CacheSignal {
    pub state: CacheSignalState,
    pub confidence: Option<CacheSignalConfidence>,
    pub reason: Option<CacheSignalReason>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TelemetryWarning {
    pub code: TelemetryWarningCode,
    pub count: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TelemetryWarningSummary {
    pub total: u64,
    pub items: Vec<TelemetryWarning>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TelemetrySnapshot {
    pub schema_version: u64,
    pub state: TelemetryState,
    pub reason: Option<String>,
    pub content_mode: TelemetryContentMode,
    pub source_version: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub session: UsageBreakdown,
    pub latest_call: Option<ModelCallSummary>,
    pub last_interaction: Option<UsageBreakdown>,
    pub context: Option<ContextUsage>,
    pub counts: ActivityCounts,
    pub subagents: SubagentBreakdown,
    pub cache_signal: Option<CacheSignal>,
    pub reporting: ReportingCoverage,
    pub warnings: TelemetryWarningSummary,
}

impl Default for TelemetrySnapshot {
    fn default() -> Self {
        Self {
            schema_version: 1,
            state: TelemetryState::Unavailable,
            reason: None,
            content_mode: TelemetryContentMode::Metadata,
            source_version: None,
            observed_at: None,
            received_at: None,
            session: UsageBreakdown::default(),
            latest_call: None,
            last_interaction: None,
            context: None,
            counts: ActivityCounts::default(),
            subagents: SubagentBreakdown::default(),
            cache_signal: None,
            reporting: ReportingCoverage::default(),
            warnings: TelemetryWarningSummary::default(),
        }
    }
}

impl TelemetrySnapshot {
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            state: TelemetryState::Unavailable,
            reason: Some(reason.into()),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub harness: HarnessName,
    pub interaction_mode: InteractionMode,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupResourceKind {
    KernelContainer,
    SessionWorkspaceVolume,
    SessionTelemetryVolume,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CleanupResourceIdentity {
    pub kind: CleanupResourceKind,
    pub name: String,
    pub resource_id: String,
    pub session_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupAction {
    WouldDelete,
    Deleted,
    DeleteFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupResource {
    pub kind: CleanupResourceKind,
    pub name: String,
    pub resource_id: String,
    pub session_id: Option<String>,
    pub interaction_mode: Option<String>,
    pub status: Option<String>,
    pub action: CleanupAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CleanupReport {
    pub dry_run: bool,
    pub owned_session_count: usize,
    pub resources: Vec<CleanupResource>,
    pub deleted_count: usize,
    pub error_count: usize,
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

    use super::{
        ActivityCounts, CacheReportingState, ContextUsage, KernelEvent, KernelEventType,
        KernelStatus, ModelCallSummary, ReportingCoverage, TelemetryContentMode, TelemetrySnapshot,
        TelemetryState, TelemetryWarning, TelemetryWarningCode, TelemetryWarningSummary,
        TokenAccountingConvention, UsageBreakdown, WorkspaceMount, WorkspaceMountMode,
    };

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

    #[test]
    #[allow(clippy::too_many_lines)]
    fn telemetry_snapshot_serialization_matches_kernel_shape() {
        let snapshot = TelemetrySnapshot {
            schema_version: 1,
            state: TelemetryState::Live,
            reason: None,
            content_mode: TelemetryContentMode::Metadata,
            source_version: Some("1.0.81-0".to_owned()),
            observed_at: Some("2026-08-15T00:00:00Z".to_owned()),
            received_at: Some("2026-08-15T00:00:01Z".to_owned()),
            session: UsageBreakdown {
                raw_input_tokens: Some(12),
                effective_input_tokens: Some(9),
                output_tokens: Some(3),
                total_tokens: Some(15),
                reasoning_output_tokens: Some(1),
                cache_read_input_tokens: Some(2),
                cache_write_input_tokens: Some(1),
                other_input_tokens: Some(5),
                fresh_input_tokens: Some(7),
                cache_reuse_percent: Some(22.5),
                nano_aiu: Some(8),
                opaque_cost: Some(0.5),
            },
            latest_call: Some(ModelCallSummary {
                started_at: Some("2026-08-15T00:00:00Z".to_owned()),
                ended_at: Some("2026-08-15T00:00:01Z".to_owned()),
                duration_ms: Some(1_000),
                model: Some("gpt-5.6-sol".to_owned()),
                requested_model: Some("gpt-5.6-sol".to_owned()),
                provider: Some("openai".to_owned()),
                agent_id: Some("builtin:task".to_owned()),
                agent_name: Some("task".to_owned()),
                is_subagent: true,
                cache_reporting: CacheReportingState::Reported,
                token_accounting_convention: TokenAccountingConvention::Inclusive,
                usage: UsageBreakdown {
                    raw_input_tokens: Some(6),
                    effective_input_tokens: Some(4),
                    output_tokens: Some(2),
                    total_tokens: Some(8),
                    reasoning_output_tokens: Some(1),
                    cache_read_input_tokens: Some(2),
                    cache_write_input_tokens: Some(1),
                    other_input_tokens: Some(1),
                    fresh_input_tokens: Some(3),
                    cache_reuse_percent: Some(33.3),
                    nano_aiu: Some(4),
                    opaque_cost: Some(0.25),
                },
            }),
            last_interaction: Some(UsageBreakdown {
                raw_input_tokens: Some(10),
                effective_input_tokens: Some(8),
                output_tokens: Some(3),
                total_tokens: Some(13),
                reasoning_output_tokens: Some(1),
                cache_read_input_tokens: Some(2),
                cache_write_input_tokens: Some(1),
                other_input_tokens: Some(5),
                fresh_input_tokens: Some(6),
                cache_reuse_percent: Some(20.0),
                nano_aiu: Some(6),
                opaque_cost: Some(0.4),
            }),
            context: Some(ContextUsage {
                tokens: Some(111),
                limit: Some(222),
                message_count: Some(3),
                observed_at: Some("2026-08-15T00:00:00Z".to_owned()),
            }),
            counts: ActivityCounts {
                interactions: 1,
                model_calls: 2,
                tool_calls: 3,
                subagent_invocations: 4,
                subagent_model_calls: 5,
                errors: 6,
            },
            subagents: super::SubagentBreakdown {
                invocations: 1,
                model_calls: 2,
                effective_input_tokens: Some(3),
                output_tokens: Some(4),
                cache_read_input_tokens: Some(5),
                cache_write_input_tokens: Some(6),
                duration_ms: Some(7),
            },
            cache_signal: Some(super::CacheSignal {
                state: super::CacheSignalState::CacheResetSuspected,
                confidence: Some(super::CacheSignalConfidence::Medium),
                reason: Some(super::CacheSignalReason::ContextDiscontinuity),
            }),
            reporting: ReportingCoverage {
                model_calls: 2,
                cache_reported_calls: 1,
                convention_resolved_calls: 2,
                effective_input_covered_calls: 2,
                context_reported: true,
            },
            warnings: TelemetryWarningSummary {
                total: 2,
                items: vec![TelemetryWarning {
                    code: TelemetryWarningCode::MalformedRecord,
                    count: 2,
                }],
            },
        };

        let payload = serde_json::to_value(snapshot)
            .unwrap_or_else(|error| panic!("failed to serialize telemetry snapshot: {error}"));

        assert_eq!(payload["schema_version"], 1);
        assert_eq!(payload["state"], "live");
        assert_eq!(payload["content_mode"], "metadata");
        assert_eq!(payload["latest_call"]["cache_reporting"], "reported");
        assert_eq!(
            payload["latest_call"]["token_accounting_convention"],
            "inclusive"
        );
        assert_eq!(payload["cache_signal"]["state"], "cache_reset_suspected");
        assert_eq!(payload["warnings"]["items"][0]["code"], "malformed_record");
    }
}
