use std::{
    collections::BTreeMap,
    fmt::{self, Display, Formatter},
    str::FromStr,
};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{config::value::SecretName, errors::ValidationError};

pub const DEFAULT_CONNECTION_API_FLAVOR: ConnectionApiFlavor = ConnectionApiFlavor::ChatCompletions;
pub const DEFAULT_AGENT_SYSTEM_PROMPT: &str = "You are a helpful assistant. Despite living inside a coding agent harness, you are not strictly a coding assistant. Instead, you help the user with any and all tasks they give you (possibly including coding!) using the tools and skills at your disposal. Pro tip: always prefer your skills and tools over generic CLI tools (though you can use those, too!)";

#[must_use]
pub fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl MessageRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
}

impl Display for MessageRole {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientType {
    Cli,
    Webui,
}

impl ClientType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Webui => "webui",
        }
    }
}

impl Display for ClientType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessName {
    Acp,
    CopilotCli,
    Codex,
    Opencode,
    ClaudeCode,
    Echo,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CliHarnessName {
    CopilotCli,
}

impl CliHarnessName {
    const ALL: [Self; 1] = [Self::CopilotCli];

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &Self::ALL
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CopilotCli => "copilot-cli",
        }
    }
}

impl Display for CliHarnessName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CliHarnessName {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "copilot-cli" => Ok(Self::CopilotCli),
            _ => Err(ValidationError::InvalidCliHarnessName {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCliRecord {
    pub harness: CliHarnessName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
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

impl FromStr for InteractionMode {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "chat" => Ok(Self::Chat),
            "cli" => Ok(Self::Cli),
            _ => Err(ValidationError::InvalidInteractionMode {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeStatus {
    Starting,
    Live,
    Exited,
    Disconnected,
    Resuming,
    Error,
}

impl RuntimeStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Live => "live",
            Self::Exited => "exited",
            Self::Disconnected => "disconnected",
            Self::Resuming => "resuming",
            Self::Error => "error",
        }
    }
}

impl FromStr for RuntimeStatus {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "starting" => Ok(Self::Starting),
            "live" => Ok(Self::Live),
            "exited" => Ok(Self::Exited),
            "disconnected" => Ok(Self::Disconnected),
            "resuming" => Ok(Self::Resuming),
            "error" => Ok(Self::Error),
            _ => Err(ValidationError::InvalidRuntimeStatus {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryState {
    Recoverable,
    LegacyUnrecoverable,
}

impl RecoveryState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recoverable => "recoverable",
            Self::LegacyUnrecoverable => "legacy-unrecoverable",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaunchValueSource {
    Literal { value: String },
    ConfigReference { field: String },
    SecretReference { field: String, name: SecretName },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliProviderLaunchSnapshot {
    pub provider_type: String,
    pub wire_api: String,
    pub connection_id: String,
    pub base_url: LaunchValueSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<LaunchValueSource>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdditionalPathIdentity {
    SessionWorkspace {
        path: String,
    },
    MountedWorkspace {
        workspace_id: String,
        mode: WorkspaceMountMode,
        path: String,
    },
    Configured {
        source: LaunchValueSource,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentProfileLaunchSnapshot {
    pub identity: String,
    pub system_prompt: LaunchValueSource,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliLaunchOptionsSnapshot {
    pub no_auto_update: bool,
    pub mouse: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<LaunchValueSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_args: Option<LaunchValueSource>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliLaunchSnapshot {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<CliProviderLaunchSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<LaunchValueSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<LaunchValueSource>,
    pub options: CliLaunchOptionsSnapshot,
    pub additional_paths: Vec<AdditionalPathIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_profile: Option<AgentProfileLaunchSnapshot>,
}

impl HarnessName {
    const ALL: [Self; 6] = [
        Self::ClaudeCode,
        Self::Echo,
        Self::CopilotCli,
        Self::Codex,
        Self::Opencode,
        Self::Acp,
    ];

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &Self::ALL
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acp => "acp",
            Self::CopilotCli => "copilot-cli",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::ClaudeCode => "claude-code",
            Self::Echo => "echo",
        }
    }
}

impl Display for HarnessName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for HarnessName {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "acp" => Ok(Self::Acp),
            "copilot-cli" => Ok(Self::CopilotCli),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::Opencode),
            "claude-code" => Ok(Self::ClaudeCode),
            "echo" => Ok(Self::Echo),
            _ => Err(ValidationError::InvalidHarnessName {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GatewayType {
    Echo,
    Discord,
}

impl GatewayType {
    const ALL: [Self; 2] = [Self::Echo, Self::Discord];

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &Self::ALL
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::Discord => "discord",
        }
    }
}

impl Display for GatewayType {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for GatewayType {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "echo" => Ok(Self::Echo),
            "discord" => Ok(Self::Discord),
            _ => Err(ValidationError::InvalidGatewayType {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionApiFlavor {
    #[default]
    ChatCompletions,
    Responses,
}

impl ConnectionApiFlavor {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
        }
    }
}

impl Display for ConnectionApiFlavor {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ConnectionApiFlavor {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "chat_completions" => Ok(Self::ChatCompletions),
            "responses" => Ok(Self::Responses),
            _ => Err(ValidationError::InvalidConnectionApiFlavor {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolCallRecord {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_offset: Option<usize>,
}

impl ToolCallRecord {
    #[must_use]
    pub fn new(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            tool_call_id: None,
            status: None,
            kind: None,
            input: None,
            output: None,
            content_offset: None,
        }
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        let mut data = Map::new();
        data.insert("tool".to_owned(), json!(self.tool));
        insert_optional(&mut data, "tool_call_id", self.tool_call_id.as_deref());
        insert_optional(&mut data, "status", self.status.as_deref());
        insert_optional(&mut data, "kind", self.kind.as_deref());
        insert_optional(&mut data, "input", self.input.as_deref());
        insert_optional(&mut data, "output", self.output.as_deref());
        if let Some(content_offset) = self.content_offset {
            data.insert("content_offset".to_owned(), json!(content_offset));
        }
        Value::Object(data)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceMountRecord {
    pub workspace_id: String,
    #[serde(default)]
    pub mode: WorkspaceMountMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_name: Option<String>,
}

impl WorkspaceMountRecord {
    #[must_use]
    pub fn new(workspace_id: impl Into<String>, mode: WorkspaceMountMode) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            mode,
            volume_name: None,
        }
    }

    #[must_use]
    pub fn mount_path(&self) -> String {
        format!("/workspace/{}", self.workspace_id)
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        json!({
            "workspace_id": self.workspace_id,
            "mode": self.mode.as_str(),
            "mount_path": self.mount_path(),
            "volume_name": self.volume_name.as_deref(),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMountMode {
    #[serde(rename = "rw")]
    #[default]
    ReadWrite,
    #[serde(rename = "ro")]
    ReadOnly,
}

impl WorkspaceMountMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadWrite => "rw",
            Self::ReadOnly => "ro",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceRecord {
    pub workspace_id: String,
    pub name: String,
    #[serde(default)]
    pub status: WorkspaceStatus,
    pub created_at: String,
    pub updated_at: String,
}

impl WorkspaceRecord {
    #[must_use]
    pub fn new(workspace_id: impl Into<String>, name: impl Into<String>) -> Self {
        let now = utc_now();
        Self {
            workspace_id: workspace_id.into(),
            name: name.into(),
            status: WorkspaceStatus::Ready,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[must_use]
    pub fn new_with_status(
        workspace_id: impl Into<String>,
        name: impl Into<String>,
        status: WorkspaceStatus,
    ) -> Self {
        let now = utc_now();
        Self {
            workspace_id: workspace_id.into(),
            name: name.into(),
            status,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[must_use]
    pub fn volume_name(&self) -> String {
        format!("agentspace-workspace-{}", self.workspace_id)
    }

    #[must_use]
    pub fn mount_path(&self) -> String {
        format!("/workspace/{}", self.workspace_id)
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        json!({
            "workspace_id": self.workspace_id,
            "name": self.name,
            "status": self.status.as_str(),
            "mount_path": self.mount_path(),
            "volume_name": self.volume_name(),
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceStatus {
    Creating,
    #[default]
    Ready,
    Failed,
}

impl WorkspaceStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}

impl Display for WorkspaceStatus {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for WorkspaceStatus {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "creating" => Ok(Self::Creating),
            "ready" => Ok(Self::Ready),
            "failed" => Ok(Self::Failed),
            _ => Err(ValidationError::InvalidWorkspaceStatus {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRecord {
    pub agent_id: String,
    pub name: String,
    pub harness: HarnessName,
    pub system_prompt: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub env_vars: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<AgentCliRecord>,
    #[serde(default)]
    pub workspace_mounts: Vec<WorkspaceMountRecord>,
    pub created_at: String,
    pub updated_at: String,
}

impl AgentRecord {
    #[must_use]
    pub fn new(
        agent_id: impl Into<String>,
        name: impl Into<String>,
        harness: HarnessName,
        system_prompt: impl Into<String>,
    ) -> Self {
        let now = utc_now();
        Self {
            agent_id: agent_id.into(),
            name: name.into(),
            harness,
            system_prompt: system_prompt.into(),
            skills: Vec::new(),
            env_vars: String::new(),
            connection_id: None,
            cli: None,
            workspace_mounts: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        json!({
            "agent_id": self.agent_id,
            "name": self.name,
            "harness": self.harness.as_str(),
            "system_prompt": self.system_prompt,
            "skills": self.skills,
            "env_vars": self.env_vars,
            "connection_id": self.connection_id,
            "cli": self.cli.as_ref().map(|cli| json!({
                "harness": cli.harness.as_str(),
                "connection_id": cli.connection_id,
            })),
            "workspace_mounts": self.workspace_mounts.iter().map(WorkspaceMountRecord::summary).collect::<Vec<_>>(),
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct KernelConfigRecord {
    pub harness: HarnessName,
    #[serde(default)]
    pub env_vars: String,
    pub updated_at: String,
}

impl KernelConfigRecord {
    #[must_use]
    pub fn new(harness: HarnessName, env_vars: impl Into<String>) -> Self {
        Self {
            harness,
            env_vars: env_vars.into(),
            updated_at: utc_now(),
        }
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        json!({
            "harness": self.harness.as_str(),
            "env_vars": self.env_vars,
            "updated_at": self.updated_at,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MessageRecord {
    pub message_id: String,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRecord>,
    #[serde(default)]
    pub reasoning: String,
}

impl MessageRecord {
    #[must_use]
    pub fn new(
        message_id: impl Into<String>,
        session_id: impl Into<String>,
        role: MessageRole,
        content: impl Into<String>,
    ) -> Self {
        Self {
            message_id: message_id.into(),
            session_id: session_id.into(),
            role,
            content: content.into(),
            created_at: utc_now(),
            tool_calls: Vec::new(),
            reasoning: String::new(),
        }
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        let mut data = Map::new();
        data.insert("message_id".to_owned(), json!(self.message_id));
        data.insert("session_id".to_owned(), json!(self.session_id));
        data.insert("role".to_owned(), json!(self.role.as_str()));
        data.insert("content".to_owned(), json!(self.content));
        data.insert("created_at".to_owned(), json!(self.created_at));
        if !self.tool_calls.is_empty() {
            let tool_calls = self
                .tool_calls
                .iter()
                .map(ToolCallRecord::summary)
                .collect::<Vec<_>>();
            data.insert("tool_calls".to_owned(), json!(tool_calls));
        }
        if !self.reasoning.is_empty() {
            data.insert("reasoning".to_owned(), json!(self.reasoning));
        }
        Value::Object(data)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub agent_id: String,
    pub agent_host_session_id: String,
    pub status: String,
    pub channel_name: Option<String>,
    pub client_type: Option<ClientType>,
    #[serde(default)]
    pub interaction_mode: InteractionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_harness: Option<CliHarnessName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_status: Option<RuntimeStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_volume_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_volume_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_mounts: Vec<WorkspaceMountRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_snapshot: Option<CliLaunchSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vscode_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_port_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub messages: Vec<MessageRecord>,
}

impl SessionRecord {
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        agent_id: impl Into<String>,
        agent_host_session_id: impl Into<String>,
        status: impl Into<String>,
        channel_name: Option<String>,
        client_type: Option<ClientType>,
    ) -> Self {
        let now = utc_now();
        Self {
            session_id: session_id.into(),
            agent_id: agent_id.into(),
            agent_host_session_id: agent_host_session_id.into(),
            status: status.into(),
            channel_name,
            client_type,
            interaction_mode: InteractionMode::Chat,
            cli_harness: None,
            cli_connection_id: None,
            harness_session_id: None,
            runtime_generation: None,
            runtime_status: None,
            workspace_volume_identity: None,
            telemetry_volume_identity: None,
            workspace_mounts: Vec::new(),
            launch_snapshot: None,
            vscode_url: None,
            free_port_url: None,
            created_at: now.clone(),
            updated_at: now,
            messages: Vec::new(),
        }
    }

    #[must_use]
    pub const fn recovery_state(&self) -> RecoveryState {
        if self.workspace_volume_identity.is_some() {
            RecoveryState::Recoverable
        } else {
            RecoveryState::LegacyUnrecoverable
        }
    }

    #[must_use]
    pub fn summary(&self) -> Value {
        serde_json::to_value(PublicSessionSummary::from(self))
            .unwrap_or_else(|_| Value::Object(Map::new()))
    }

    #[must_use]
    pub fn detail(&self) -> Value {
        let mut data = match self.summary() {
            Value::Object(data) => data,
            _ => Map::new(),
        };
        let messages = self
            .messages
            .iter()
            .map(MessageRecord::summary)
            .collect::<Vec<_>>();
        data.insert("messages".to_owned(), json!(messages));
        Value::Object(data)
    }
}

#[derive(Serialize)]
pub struct PublicSessionSummary<'a> {
    pub session_id: &'a str,
    pub agent_id: &'a str,
    pub status: &'a str,
    pub channel_name: Option<&'a str>,
    pub client_type: Option<&'static str>,
    pub interaction_mode: &'static str,
    pub cli_harness: Option<&'static str>,
    pub cli_connection_id: Option<&'a str>,
    pub harness_session_id: Option<&'a str>,
    pub runtime_generation: Option<u64>,
    pub runtime_status: Option<&'static str>,
    pub recovery_state: &'static str,
    pub telemetry_volume_identity: Option<&'a str>,
    pub vscode_url: Option<&'a str>,
    pub free_port_url: Option<&'a str>,
    pub created_at: &'a str,
    pub updated_at: &'a str,
    pub message_count: usize,
}

impl<'a> From<&'a SessionRecord> for PublicSessionSummary<'a> {
    fn from(session: &'a SessionRecord) -> Self {
        Self {
            session_id: &session.session_id,
            agent_id: &session.agent_id,
            status: &session.status,
            channel_name: session.channel_name.as_deref(),
            client_type: session.client_type.map(ClientType::as_str),
            interaction_mode: session.interaction_mode.as_str(),
            cli_harness: session.cli_harness.map(CliHarnessName::as_str),
            cli_connection_id: session.cli_connection_id.as_deref(),
            harness_session_id: session.harness_session_id.as_deref(),
            runtime_generation: session.runtime_generation,
            runtime_status: session.runtime_status.map(RuntimeStatus::as_str),
            recovery_state: session.recovery_state().as_str(),
            telemetry_volume_identity: session.telemetry_volume_identity.as_deref(),
            vscode_url: session.vscode_url.as_deref(),
            free_port_url: session.free_port_url.as_deref(),
            created_at: &session.created_at,
            updated_at: &session.updated_at,
            message_count: session.messages.len(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicTerminalStatus {
    pub state: String,
    pub exit_status: Option<i64>,
    pub attach_kind: Option<String>,
    pub attachment_count: usize,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConnectionRecord {
    pub connection_id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub api_flavor: ConnectionApiFlavor,
    /// Literal API key. Only authorable through YAML; mutually exclusive with
    /// [`ConnectionRecord::api_key_secret`].
    #[serde(default)]
    pub api_key: String,
    /// Declared secret backing the API key, when the configured value is a
    /// `secretRef` rather than a literal. Typed as a [`SecretName`] so an
    /// invalid name cannot be represented and silently dropped on projection.
    #[serde(default)]
    pub api_key_secret: Option<SecretName>,
    pub created_at: String,
    pub updated_at: String,
}

impl ConnectionRecord {
    #[must_use]
    pub fn new(
        connection_id: impl Into<String>,
        name: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        let now = utc_now();
        Self {
            connection_id: connection_id.into(),
            name: name.into(),
            url: url.into(),
            api_flavor: DEFAULT_CONNECTION_API_FLAVOR,
            api_key: String::new(),
            api_key_secret: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Whether an API key is configured, either as a literal or as a
    /// `secretRef`. This says nothing about whether a referenced secret has a
    /// value set.
    #[must_use]
    pub const fn has_api_key(&self) -> bool {
        !self.api_key.is_empty() || self.api_key_secret.is_some()
    }

    #[must_use]
    pub fn summary(&self, include_api_key: bool) -> Value {
        let mut data = Map::new();
        data.insert("connection_id".to_owned(), json!(self.connection_id));
        data.insert("name".to_owned(), json!(self.name));
        data.insert("url".to_owned(), json!(self.url));
        data.insert("api_flavor".to_owned(), json!(self.api_flavor.as_str()));
        data.insert("has_api_key".to_owned(), json!(self.has_api_key()));
        data.insert("api_key_secret".to_owned(), json!(self.api_key_secret));
        data.insert("created_at".to_owned(), json!(self.created_at));
        data.insert("updated_at".to_owned(), json!(self.updated_at));
        if include_api_key {
            data.insert("api_key".to_owned(), json!(self.api_key));
        }
        Value::Object(data)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GatewayRecord {
    pub gateway_id: String,
    pub name: String,
    pub gateway_type: GatewayType,
    pub agent_id: String,
    pub enabled: bool,
    #[serde(default)]
    pub env_vars: String,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default = "default_gateway_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl GatewayRecord {
    #[must_use]
    pub fn new(
        gateway_id: impl Into<String>,
        name: impl Into<String>,
        gateway_type: GatewayType,
        agent_id: impl Into<String>,
        enabled: bool,
    ) -> Self {
        let now = utc_now();
        Self {
            gateway_id: gateway_id.into(),
            name: name.into(),
            gateway_type,
            agent_id: agent_id.into(),
            enabled,
            env_vars: String::new(),
            secrets: BTreeMap::new(),
            status: default_gateway_status(),
            last_error: None,
            container_name: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[must_use]
    pub fn summary(&self, include_secrets: bool) -> Value {
        let secret_keys = self.secrets.keys().cloned().collect::<Vec<_>>();
        let mut data = Map::new();
        data.insert("gateway_id".to_owned(), json!(self.gateway_id));
        data.insert("name".to_owned(), json!(self.name));
        data.insert("gateway_type".to_owned(), json!(self.gateway_type.as_str()));
        data.insert("agent_id".to_owned(), json!(self.agent_id));
        data.insert("enabled".to_owned(), json!(self.enabled));
        data.insert("env_vars".to_owned(), json!(self.env_vars));
        data.insert("status".to_owned(), json!(self.status));
        data.insert("last_error".to_owned(), json!(self.last_error));
        data.insert("container_name".to_owned(), json!(self.container_name));
        data.insert("created_at".to_owned(), json!(self.created_at));
        data.insert("updated_at".to_owned(), json!(self.updated_at));
        data.insert("secret_keys".to_owned(), json!(secret_keys));
        if include_secrets {
            data.insert("secrets".to_owned(), json!(self.secrets));
        }
        Value::Object(data)
    }

    #[must_use]
    pub fn effective_env(&self) -> BTreeMap<String, String> {
        let mut env = parse_env_vars(&self.env_vars);
        env.extend(self.secrets.clone());
        env
    }
}

#[must_use]
pub fn parse_env_vars(raw: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').map_or((line, ""), |parts| parts);
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = trim_matching_quotes(value.trim());
        env.insert(key.to_owned(), value.to_owned());
    }
    env
}

pub fn validate_agent_id(agent_id: &str) -> Result<(), ValidationError> {
    validate_alpha_dash_id(agent_id)
        .then_some(())
        .ok_or_else(|| ValidationError::InvalidAgentId {
            value: agent_id.to_owned(),
        })
}

pub fn validate_connection_id(connection_id: &str) -> Result<(), ValidationError> {
    validate_alpha_dash_id(connection_id)
        .then_some(())
        .ok_or_else(|| ValidationError::InvalidConnectionId {
            value: connection_id.to_owned(),
        })
}

pub fn validate_gateway_id(gateway_id: &str) -> Result<(), ValidationError> {
    validate_alpha_dash_id(gateway_id)
        .then_some(())
        .ok_or_else(|| ValidationError::InvalidGatewayId {
            value: gateway_id.to_owned(),
        })
}

pub fn validate_skill_id(skill_id: &str) -> Result<(), ValidationError> {
    validate_alphanumeric_dash_id(skill_id)
        .then_some(())
        .ok_or_else(|| ValidationError::InvalidSkillId {
            value: skill_id.to_owned(),
        })
}

pub fn validate_workspace_id(workspace_id: &str) -> Result<(), ValidationError> {
    validate_alphanumeric_dash_id(workspace_id)
        .then_some(())
        .ok_or_else(|| ValidationError::InvalidWorkspaceId {
            value: workspace_id.to_owned(),
        })
}

#[must_use]
fn default_gateway_status() -> String {
    "stopped".to_owned()
}

fn insert_optional(data: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        data.insert(key.to_owned(), json!(value));
    }
}

#[must_use]
fn trim_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[must_use]
fn validate_alpha_dash_id(value: &str) -> bool {
    validate_dash_id(value, |byte| byte.is_ascii_lowercase())
}

#[must_use]
fn validate_alphanumeric_dash_id(value: &str) -> bool {
    validate_dash_id(value, |byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit()
    })
}

#[must_use]
fn validate_dash_id(value: &str, is_allowed: impl Fn(u8) -> bool) -> bool {
    !value.is_empty()
        && value
            .split('-')
            .all(|segment| !segment.is_empty() && segment.bytes().all(&is_allowed))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, error::Error, str::FromStr};

    use serde_json::json;

    use super::{
        ActivityCounts, AgentCliRecord, AgentRecord, CacheReportingState, CliHarnessName,
        ClientType, ConnectionApiFlavor, ConnectionRecord, ContextUsage, GatewayRecord,
        GatewayType, HarnessName, InteractionMode, KernelConfigRecord, MessageRecord, MessageRole,
        ModelCallSummary, ReportingCoverage, SecretName, TelemetryContentMode, TelemetrySnapshot,
        TelemetryState, TelemetryWarning, TelemetryWarningCode, TelemetryWarningSummary,
        TokenAccountingConvention, ToolCallRecord, UsageBreakdown, parse_env_vars,
        validate_agent_id, validate_connection_id, validate_gateway_id, validate_skill_id,
        validate_workspace_id,
    };

    #[test]
    fn tool_call_summary_omits_absent_optional_fields() {
        let mut tool_call = ToolCallRecord::new("shell");
        tool_call.status = Some("done".to_owned());
        tool_call.content_offset = Some(7);

        assert_eq!(
            tool_call.summary(),
            json!({
                "tool": "shell",
                "status": "done",
                "content_offset": 7,
            })
        );
    }

    #[test]
    fn message_summary_omits_empty_tool_calls_and_reasoning() {
        let mut message = MessageRecord::new("msg", "session", MessageRole::Assistant, "hello");
        message.created_at = "2024-01-01T00:00:00+00:00".to_owned();

        assert_eq!(
            message.summary(),
            json!({
                "message_id": "msg",
                "session_id": "session",
                "role": "assistant",
                "content": "hello",
                "created_at": "2024-01-01T00:00:00+00:00",
            })
        );
    }

    #[test]
    fn message_summary_includes_non_empty_tool_calls_and_reasoning() {
        let mut message = MessageRecord::new("msg", "session", MessageRole::Assistant, "hello");
        message.created_at = "2024-01-01T00:00:00+00:00".to_owned();
        message.reasoning = "because".to_owned();
        message.tool_calls.push(ToolCallRecord::new("shell"));

        assert_eq!(
            message.summary(),
            json!({
                "message_id": "msg",
                "session_id": "session",
                "role": "assistant",
                "content": "hello",
                "created_at": "2024-01-01T00:00:00+00:00",
                "tool_calls": [{ "tool": "shell" }],
                "reasoning": "because",
            })
        );
    }

    #[test]
    fn record_summaries_match_python_shapes() {
        let mut agent = AgentRecord::new("agent", "Agent", HarnessName::Acp, "prompt");
        agent.created_at = "c".to_owned();
        agent.updated_at = "u".to_owned();
        agent.skills = vec!["skill".to_owned()];
        agent.env_vars = "A=B".to_owned();
        agent.connection_id = Some("conn".to_owned());
        agent.cli = Some(AgentCliRecord {
            harness: CliHarnessName::CopilotCli,
            connection_id: Some("cli-conn".to_owned()),
        });
        assert_eq!(
            agent.summary(),
            json!({
                "agent_id": "agent",
                "name": "Agent",
                "harness": "acp",
                "system_prompt": "prompt",
                "skills": ["skill"],
                "env_vars": "A=B",
                "connection_id": "conn",
                "cli": {
                    "harness": "copilot-cli",
                    "connection_id": "cli-conn",
                },
                "workspace_mounts": [],
                "created_at": "c",
                "updated_at": "u",
            })
        );

        let mut config = KernelConfigRecord::new(HarnessName::Echo, "X=1");
        config.updated_at = "u".to_owned();
        assert_eq!(
            config.summary(),
            json!({"harness": "echo", "env_vars": "X=1", "updated_at": "u"})
        );

        let mut session = super::SessionRecord::new(
            "sess",
            "agent",
            "host-sess",
            "running",
            Some("cli".to_owned()),
            Some(ClientType::Cli),
        );
        session.created_at = "c".to_owned();
        session.updated_at = "u".to_owned();
        session
            .messages
            .push(MessageRecord::new("msg", "sess", MessageRole::User, "hi"));
        assert_eq!(
            session.summary(),
            json!({
                "session_id": "sess",
                "agent_id": "agent",
                "status": "running",
                "channel_name": "cli",
                "client_type": "cli",
                "interaction_mode": "chat",
                "cli_harness": null,
                "cli_connection_id": null,
                "harness_session_id": null,
                "runtime_generation": null,
                "runtime_status": null,
                "recovery_state": "legacy-unrecoverable",
                "telemetry_volume_identity": null,
                "vscode_url": null,
                "free_port_url": null,
                "created_at": "c",
                "updated_at": "u",
                "message_count": 1,
            })
        );
    }

    #[test]
    fn cli_harness_and_interaction_mode_are_strict() {
        assert_eq!(
            CliHarnessName::from_str("copilot-cli"),
            Ok(CliHarnessName::CopilotCli)
        );
        assert!(CliHarnessName::from_str("acp").is_err());
        assert_eq!(InteractionMode::from_str("chat"), Ok(InteractionMode::Chat));
        assert_eq!(InteractionMode::from_str("cli"), Ok(InteractionMode::Cli));
        assert!(InteractionMode::from_str("terminal").is_err());
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

    #[test]
    #[allow(clippy::too_many_lines)]
    fn telemetry_snapshot_deserialization_accepts_sanitized_null_numeric_fields() {
        let payload = json!({
            "schema_version": 1,
            "state": "degraded",
            "reason": "invalid_usage_shape",
            "content_mode": "metadata",
            "source_version": "1.0.81-0",
            "observed_at": "2026-08-15T00:00:00Z",
            "received_at": "2026-08-15T00:00:01Z",
            "session": {
                "raw_input_tokens": null,
                "effective_input_tokens": null,
                "output_tokens": null,
                "total_tokens": null,
                "reasoning_output_tokens": null,
                "cache_read_input_tokens": null,
                "cache_write_input_tokens": null,
                "other_input_tokens": null,
                "fresh_input_tokens": null,
                "cache_reuse_percent": null,
                "nano_aiu": null,
                "opaque_cost": null
            },
            "latest_call": {
                "started_at": "2026-08-15T00:00:00Z",
                "ended_at": "2026-08-15T00:00:01Z",
                "duration_ms": null,
                "model": "gpt-5.6-sol",
                "requested_model": "gpt-5.6-sol",
                "provider": "openai",
                "agent_id": null,
                "agent_name": null,
                "is_subagent": false,
                "cache_reporting": "unreported",
                "token_accounting_convention": "unknown",
                "usage": {
                    "raw_input_tokens": null,
                    "effective_input_tokens": null,
                    "output_tokens": null,
                    "total_tokens": null,
                    "reasoning_output_tokens": null,
                    "cache_read_input_tokens": null,
                    "cache_write_input_tokens": null,
                    "other_input_tokens": null,
                    "fresh_input_tokens": null,
                    "cache_reuse_percent": null,
                    "nano_aiu": null,
                    "opaque_cost": null
                }
            },
            "last_interaction": null,
            "context": {
                "tokens": null,
                "limit": null,
                "message_count": 3,
                "observed_at": "2026-08-15T00:00:00Z"
            },
            "counts": {
                "interactions": 1,
                "model_calls": 1,
                "tool_calls": 0,
                "subagent_invocations": 0,
                "subagent_model_calls": 0,
                "errors": 0
            },
            "subagents": {
                "invocations": 0,
                "model_calls": 0,
                "effective_input_tokens": null,
                "output_tokens": null,
                "cache_read_input_tokens": null,
                "cache_write_input_tokens": null,
                "duration_ms": null
            },
            "cache_signal": {
                "state": "unknown",
                "confidence": null,
                "reason": null
            },
            "reporting": {
                "model_calls": 1,
                "cache_reported_calls": 0,
                "convention_resolved_calls": 0,
                "effective_input_covered_calls": 0,
                "context_reported": true
            },
            "warnings": {
                "total": 1,
                "items": [{
                    "code": "invalid_usage_shape",
                    "count": 1
                }]
            }
        });

        let snapshot: TelemetrySnapshot = serde_json::from_value(payload)
            .unwrap_or_else(|error| panic!("failed to deserialize telemetry snapshot: {error}"));

        assert_eq!(snapshot.state, TelemetryState::Degraded);
        assert!(snapshot.session.raw_input_tokens.is_none());
        assert_eq!(
            snapshot
                .context
                .as_ref()
                .and_then(|context| context.message_count),
            Some(3)
        );
        assert_eq!(
            snapshot.warnings.items.first().map(|warning| warning.code),
            Some(TelemetryWarningCode::InvalidUsageShape)
        );
    }

    #[test]
    fn connection_summary_hides_api_key_by_default() {
        let mut connection = ConnectionRecord::new("conn", "Connection", "http://example.test");
        connection.created_at = "c".to_owned();
        connection.updated_at = "u".to_owned();
        connection.api_key = "secret".to_owned();

        assert_eq!(
            connection.summary(false),
            json!({
                "connection_id": "conn",
                "name": "Connection",
                "url": "http://example.test",
                "api_flavor": "chat_completions",
                "has_api_key": true,
                "api_key_secret": null,
                "created_at": "c",
                "updated_at": "u",
            })
        );
        assert_eq!(
            connection.summary(true),
            json!({
                "connection_id": "conn",
                "name": "Connection",
                "url": "http://example.test",
                "api_flavor": "chat_completions",
                "has_api_key": true,
                "api_key_secret": null,
                "created_at": "c",
                "updated_at": "u",
                "api_key": "secret",
            })
        );
    }

    #[test]
    fn connection_summary_reports_secret_backed_api_key() -> Result<(), Box<dyn Error + Send + Sync>>
    {
        let mut connection = ConnectionRecord::new("conn", "Connection", "http://example.test");
        connection.created_at = "c".to_owned();
        connection.updated_at = "u".to_owned();
        connection.api_key_secret = Some(SecretName::new("OPENAI_API_KEY")?);

        assert!(connection.has_api_key());
        assert_eq!(
            connection.summary(true),
            json!({
                "connection_id": "conn",
                "name": "Connection",
                "url": "http://example.test",
                "api_flavor": "chat_completions",
                "has_api_key": true,
                "api_key_secret": "OPENAI_API_KEY",
                "created_at": "c",
                "updated_at": "u",
                "api_key": "",
            })
        );
        Ok(())
    }

    #[test]
    fn gateway_summary_sorts_secret_keys_and_hides_secret_values() {
        let mut gateway =
            GatewayRecord::new("gate", "Gateway", GatewayType::Discord, "agent", true);
        gateway.created_at = "c".to_owned();
        gateway.updated_at = "u".to_owned();
        gateway.secrets.insert("zeta".to_owned(), "last".to_owned());
        gateway
            .secrets
            .insert("alpha".to_owned(), "first".to_owned());

        assert_eq!(
            gateway.summary(false),
            json!({
                "gateway_id": "gate",
                "name": "Gateway",
                "gateway_type": "discord",
                "agent_id": "agent",
                "enabled": true,
                "env_vars": "",
                "status": "stopped",
                "last_error": null,
                "container_name": null,
                "created_at": "c",
                "updated_at": "u",
                "secret_keys": ["alpha", "zeta"],
            })
        );
        assert_eq!(
            gateway.summary(true)["secrets"],
            json!({"alpha": "first", "zeta": "last"})
        );
    }

    #[test]
    fn validation_matches_python_id_patterns() {
        assert!(validate_agent_id("alpha-beta").is_ok());
        assert!(validate_connection_id("alpha1").is_err());
        assert!(validate_gateway_id("alpha--beta").is_err());
        assert!(validate_agent_id("Alpha").is_err());

        assert!(validate_skill_id("skill-2").is_ok());
        assert!(validate_skill_id("skill--2").is_err());
        assert!(validate_skill_id("Skill").is_err());
        assert!(validate_workspace_id("workspace-2").is_ok());
        assert!(validate_workspace_id("workspace--2").is_err());
        assert!(validate_workspace_id("Workspace").is_err());
    }

    #[test]
    fn enum_parsing_accepts_contract_values() -> Result<(), Box<dyn Error + Send + Sync>> {
        assert_eq!(
            HarnessName::all()
                .iter()
                .map(|harness| harness.as_str())
                .collect::<Vec<_>>(),
            vec![
                "claude-code",
                "echo",
                "copilot-cli",
                "codex",
                "opencode",
                "acp"
            ]
        );
        assert_eq!(
            GatewayType::all()
                .iter()
                .map(|gateway_type| gateway_type.as_str())
                .collect::<Vec<_>>(),
            vec!["echo", "discord"]
        );
        assert_eq!(
            HarnessName::from_str("claude-code")?,
            HarnessName::ClaudeCode
        );
        assert_eq!(GatewayType::from_str("discord")?, GatewayType::Discord);
        assert_eq!(
            ConnectionApiFlavor::from_str("responses")?,
            ConnectionApiFlavor::Responses
        );
        assert!(HarnessName::from_str("missing").is_err());
        Ok(())
    }

    #[test]
    fn parse_env_vars_matches_python_behavior() {
        let env = parse_env_vars(
            r#"
            # comment
            A=one
            B = "two"
            C='three=four'
            NO_EQUALS
            EMPTY_KEY_IGNORED? no
            =ignored
            D= "unterminated
            "#,
        );

        let mut expected = BTreeMap::new();
        expected.insert("A".to_owned(), "one".to_owned());
        expected.insert("B".to_owned(), "two".to_owned());
        expected.insert("C".to_owned(), "three=four".to_owned());
        expected.insert("NO_EQUALS".to_owned(), String::new());
        expected.insert("EMPTY_KEY_IGNORED? no".to_owned(), String::new());
        expected.insert("D".to_owned(), "\"unterminated".to_owned());
        assert_eq!(env, expected);
    }

    #[test]
    fn gateway_effective_env_overlays_secrets() {
        let mut gateway = GatewayRecord::new("gate", "Gateway", GatewayType::Echo, "agent", true);
        gateway.env_vars = "TOKEN=from-env\nURL=https://example.test".to_owned();
        gateway
            .secrets
            .insert("TOKEN".to_owned(), "secret".to_owned());

        let mut expected = BTreeMap::new();
        expected.insert("TOKEN".to_owned(), "secret".to_owned());
        expected.insert("URL".to_owned(), "https://example.test".to_owned());
        assert_eq!(gateway.effective_env(), expected);
    }
}
