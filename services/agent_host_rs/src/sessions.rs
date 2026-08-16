use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::{Component, Path as FsPath},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::{
    AppState,
    docker_runtime::DockerKernelRuntime,
    errors::AgentHostError,
    models::{
        CleanupReport, CleanupResourceIdentity, DockerStatsSummary, HarnessName, InteractionMode,
        KernelEvent, KernelEventType, KernelRuntimeSession, KernelStatus, RuntimeSessionSummary,
        ServiceSummary, SessionSummary, TelemetrySnapshot, WorkspaceMount, WorkspaceMountMode,
    },
    skills::{SkillRegistry, SkillVolumeResource},
    terminal::{TerminalConnection, TerminalExec, TerminalService, TerminalStatus},
};

pub type EventStream = Pin<Box<dyn Stream<Item = Result<KernelEvent, AgentHostError>> + Send>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeCreateSession {
    pub session_id: String,
    pub telemetry_volume_identity: Option<String>,
    pub harness: HarnessName,
    pub interaction_mode: InteractionMode,
    pub env: BTreeMap<String, String>,
    pub additional_paths: Vec<String>,
    pub skills: Vec<String>,
    pub skill_volumes: Vec<SkillVolumeResource>,
    pub workspace_mounts: Vec<WorkspaceMount>,
}

#[async_trait]
pub trait KernelRuntime: Send + Sync {
    async fn create_session(
        &self,
        request: RuntimeCreateSession,
    ) -> Result<KernelRuntimeSession, AgentHostError>;

    async fn send_message(
        &self,
        session: KernelRuntimeSession,
        message: String,
    ) -> Result<Vec<KernelEvent>, AgentHostError> {
        let mut stream = self.stream_message(session, message)?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event?);
        }
        Ok(events)
    }

    fn stream_message(
        &self,
        session: KernelRuntimeSession,
        message: String,
    ) -> Result<EventStream, AgentHostError>;

    async fn summary(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<RuntimeSessionSummary, AgentHostError>;

    async fn history(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<Vec<Vec<KernelEvent>>, AgentHostError>;

    async fn logs(&self, session: &KernelRuntimeSession) -> Result<Vec<String>, AgentHostError>;

    async fn container_logs(
        &self,
        session: &KernelRuntimeSession,
        tail: Option<u32>,
    ) -> Result<Vec<String>, AgentHostError>;

    async fn stats(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<Option<DockerStatsSummary>, AgentHostError>;

    fn container_name(&self, session: &KernelRuntimeSession) -> Option<String>;

    fn vscode_url(&self, session: &KernelRuntimeSession) -> Option<String>;

    fn free_port_url(&self, session: &KernelRuntimeSession) -> Option<String>;

    async fn destroy_session(&self, session: KernelRuntimeSession) -> Result<(), AgentHostError>;

    async fn destroy_session_by_id(&self, session_id: &str) -> Result<(), AgentHostError>;

    async fn cleanup_orphans(
        &self,
        owned_session_ids: &BTreeSet<String>,
        dry_run: bool,
        reviewed_resources: Option<&[CleanupResourceIdentity]>,
    ) -> Result<CleanupReport, AgentHostError>;

    async fn snapshot_session_workspace(
        &self,
        session: &KernelRuntimeSession,
        workspace_id: String,
        volume_name: String,
        exclude_paths: Vec<String>,
    ) -> Result<serde_json::Value, AgentHostError>;

    async fn clone_workspace(
        &self,
        source_volume_name: String,
        target_workspace_id: String,
        target_volume_name: String,
    ) -> Result<serde_json::Value, AgentHostError>;

    async fn open_workspace_vscode(
        &self,
        workspace_id: String,
        volume_name: String,
    ) -> Result<serde_json::Value, AgentHostError>;

    async fn terminal_status(
        &self,
        _session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        Err(AgentHostError::runtime(
            "terminal status is not supported by this runtime",
        ))
    }

    async fn terminal_ensure(
        &self,
        _session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        Err(AgentHostError::runtime(
            "terminal ensure is not supported by this runtime",
        ))
    }

    async fn terminal_stop(
        &self,
        _session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        Err(AgentHostError::runtime(
            "terminal stop is not supported by this runtime",
        ))
    }

    async fn terminal_resume(
        &self,
        _session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        Err(AgentHostError::runtime(
            "terminal resume is not supported by this runtime",
        ))
    }

    async fn terminal_detach_client(
        &self,
        _session: &KernelRuntimeSession,
        tmux_client_id: &str,
    ) -> Result<TerminalStatus, AgentHostError> {
        Err(AgentHostError::terminal_attachment_not_found(
            tmux_client_id,
        ))
    }

    async fn terminal_attach(
        &self,
        _session: &KernelRuntimeSession,
        _attachment_id: &str,
        _attach_argv: &[String],
    ) -> Result<TerminalExec, AgentHostError> {
        Err(AgentHostError::runtime(
            "terminal attachment is not supported by this runtime",
        ))
    }

    async fn terminal_resize(
        &self,
        _session: &KernelRuntimeSession,
        _exec_id: &str,
        _cols: u16,
        _rows: u16,
    ) -> Result<(), AgentHostError> {
        Err(AgentHostError::runtime(
            "terminal resize is not supported by this runtime",
        ))
    }

    async fn telemetry(
        &self,
        _session: &KernelRuntimeSession,
    ) -> Result<TelemetrySnapshot, AgentHostError> {
        Ok(TelemetrySnapshot::unavailable(
            "telemetry is unavailable for this runtime",
        ))
    }
}

#[derive(Clone, Default)]
pub struct SessionRegistry {
    inner: Arc<SessionRegistryInner>,
}

struct SessionRegistryInner {
    runtime: Arc<dyn KernelRuntime>,
    skills: Option<SkillRegistry>,
    sessions: Arc<RwLock<BTreeMap<String, SessionRecord>>>,
    create_lock: Arc<Mutex<()>>,
    terminal: TerminalService,
}

impl Default for SessionRegistryInner {
    fn default() -> Self {
        let runtime: Arc<dyn KernelRuntime> = Arc::new(DockerKernelRuntime::from_env());
        Self {
            terminal: TerminalService::new(runtime.clone()),
            runtime,
            skills: None,
            sessions: Arc::new(RwLock::new(BTreeMap::new())),
            create_lock: Arc::new(Mutex::new(())),
        }
    }
}

impl SessionRegistry {
    #[must_use]
    pub fn with_runtime(runtime: Arc<dyn KernelRuntime>) -> Self {
        Self {
            inner: Arc::new(SessionRegistryInner {
                terminal: TerminalService::new(runtime.clone()),
                runtime,
                skills: None,
                sessions: Arc::new(RwLock::new(BTreeMap::new())),
                create_lock: Arc::new(Mutex::new(())),
            }),
        }
    }

    #[must_use]
    pub fn with_skills(skills: SkillRegistry) -> Self {
        Self::with_runtime_and_skills(Arc::new(DockerKernelRuntime::from_env()), skills)
    }

    #[must_use]
    pub fn with_runtime_and_skills(runtime: Arc<dyn KernelRuntime>, skills: SkillRegistry) -> Self {
        Self {
            inner: Arc::new(SessionRegistryInner {
                terminal: TerminalService::new(runtime.clone()),
                runtime,
                skills: Some(skills),
                sessions: Arc::new(RwLock::new(BTreeMap::new())),
                create_lock: Arc::new(Mutex::new(())),
            }),
        }
    }

    #[must_use]
    pub const fn summary(&self) -> ServiceSummary {
        ServiceSummary::ready("session lifecycle routes are active")
    }

    #[allow(clippy::too_many_lines)]
    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> Result<SessionSummary, AgentHostError> {
        let _create_guard = self.inner.create_lock.lock().await;
        let session_id = request
            .session_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().simple().to_string());
        validate_session_id_or_error(&session_id)?;
        if let Some(telemetry_volume_identity) = request.telemetry_volume_identity.as_deref() {
            validate_session_id_or_error(telemetry_volume_identity)?;
        }
        let existing = self.inner.sessions.read().await.get(&session_id).cloned();
        if let Some(record) = &existing {
            validate_existing_session_identity(record, &request)?;
        }
        if request.harness == HarnessName::CopilotCli && Uuid::parse_str(&session_id).is_err() {
            return Err(AgentHostError::validation(
                "Copilot session_id must be a UUID",
            ));
        }

        let caller_env = request.env;
        let mut merged_env: BTreeMap<String, String> = std::env::vars().collect();
        for key in [
            "CONNECTION_URL",
            "CONNECTION_API_KEY",
            "CONNECTION_API_FLAVOR",
        ] {
            merged_env.remove(key);
        }
        merged_env.extend(caller_env);
        let workspace_mounts = validate_workspace_mount_requests(request.workspace_mounts)?;
        let skill_volumes = match &self.inner.skills {
            Some(skills) => skills
                .resolve_volume_resources(&request.skills)
                .await
                .map_err(|error| AgentHostError::validation(error.to_string()))?,
            None => Vec::new(),
        };
        let additional_paths = append_unique_paths(
            request.additional_paths,
            workspace_mounts
                .iter()
                .map(WorkspaceMount::mount_path)
                .chain(
                    skill_volumes
                        .iter()
                        .filter(|resource| resource.advertise)
                        .map(|resource| resource.mount_path.clone()),
                )
                .collect::<Vec<_>>(),
        );
        let runtime_request = RuntimeCreateSession {
            session_id: session_id.clone(),
            telemetry_volume_identity: request.telemetry_volume_identity.clone(),
            harness: request.harness,
            interaction_mode: request.interaction_mode,
            env: merged_env.clone(),
            additional_paths: additional_paths.clone(),
            skills: request.skills.clone(),
            skill_volumes,
            workspace_mounts: workspace_mounts.clone(),
        };
        let runtime_session = self.inner.runtime.create_session(runtime_request).await?;
        if request.interaction_mode == InteractionMode::Cli {
            self.inner
                .terminal
                .reconcile_adoption(&session_id, &runtime_session)
                .await?;
        }
        let runtime_summary = self.inner.runtime.summary(&runtime_session).await?;
        let mut record = existing.unwrap_or_else(|| SessionRecord {
            session_id: session_id.clone(),
            telemetry_volume_identity: request.telemetry_volume_identity.clone(),
            harness: request.harness,
            interaction_mode: request.interaction_mode,
            runtime_session: runtime_session.clone(),
            env: BTreeMap::new(),
            additional_paths: Vec::new(),
            skills: Vec::new(),
            workspace_mounts: Vec::new(),
            history: Vec::new(),
            status: KernelStatus::Idle,
            resume_token: None,
            container_name: None,
            vscode_url: None,
            free_port_url: None,
            stats: None,
        });
        record.runtime_session = runtime_session.clone();
        record.telemetry_volume_identity = request.telemetry_volume_identity.clone();
        record.env = merged_env;
        record.additional_paths = additional_paths;
        record.skills = request.skills;
        record.workspace_mounts = workspace_mounts;
        record.container_name = self.inner.runtime.container_name(&runtime_session);
        record.vscode_url = self.inner.runtime.vscode_url(&runtime_session);
        record.free_port_url = self.inner.runtime.free_port_url(&runtime_session);
        record.apply_runtime_summary(runtime_summary);
        let summary = record.summary();
        self.inner.sessions.write().await.insert(session_id, record);
        Ok(summary)
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        message: String,
    ) -> Result<Vec<KernelEvent>, AgentHostError> {
        let mut stream = self.stream_message(session_id, message).await?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event?);
        }
        Ok(events)
    }

    pub async fn stream_message(
        &self,
        session_id: &str,
        message: String,
    ) -> Result<EventStream, AgentHostError> {
        let record = self.get_record(session_id).await?;
        let inner_stream = self
            .inner
            .runtime
            .stream_message(record.runtime_session.clone(), message)?;
        Ok(Box::pin(AgentEventStream::new(
            inner_stream,
            self.inner.runtime.clone(),
            self.inner.sessions.clone(),
            session_id.to_owned(),
            record.runtime_session,
        )))
    }

    pub async fn destroy_session(&self, session_id: &str) -> Result<(), AgentHostError> {
        let record = self.inner.sessions.read().await.get(session_id).cloned();
        if let Some(record) = record {
            self.inner.terminal.forget_session(session_id).await;
            self.inner
                .runtime
                .destroy_session(record.runtime_session)
                .await?;
            self.inner.sessions.write().await.remove(session_id);
            Ok(())
        } else {
            self.inner.runtime.destroy_session_by_id(session_id).await
        }
    }

    pub async fn destroy_all_sessions(&self) {
        let records = {
            let mut sessions = self.inner.sessions.write().await;
            std::mem::take(&mut *sessions)
        };
        for record in records.into_values() {
            if let Err(error) = self
                .inner
                .runtime
                .destroy_session(record.runtime_session)
                .await
            {
                tracing::warn!(session_id = %record.session_id, %error, "failed to destroy kernel session");
            }
        }
    }

    pub async fn forget_all_sessions(&self) {
        let session_ids = self
            .inner
            .sessions
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for session_id in session_ids {
            self.inner.terminal.forget_session(&session_id).await;
        }
        self.inner.sessions.write().await.clear();
    }

    pub async fn reset_session(&self, session_id: &str) -> Result<SessionSummary, AgentHostError> {
        let record = self.get_record(session_id).await?;
        self.destroy_session(session_id).await?;
        self.create_session(CreateSessionRequest {
            session_id: Some(record.session_id),
            telemetry_volume_identity: record.telemetry_volume_identity,
            harness: record.harness,
            interaction_mode: record.interaction_mode,
            env: record.env,
            additional_paths: record.additional_paths,
            skills: record.skills,
            workspace_mounts: record
                .workspace_mounts
                .into_iter()
                .map(WorkspaceMountRequest::from)
                .collect(),
        })
        .await
    }

    pub async fn cleanup_orphans(
        &self,
        owned_session_ids: &BTreeSet<String>,
        dry_run: bool,
        reviewed_resources: Option<&[CleanupResourceIdentity]>,
    ) -> Result<CleanupReport, AgentHostError> {
        if !dry_run && reviewed_resources.is_none() {
            return Err(AgentHostError::validation(
                "destructive runtime cleanup requires reviewed_resources from a dry-run report",
            ));
        }
        self.inner
            .runtime
            .cleanup_orphans(owned_session_ids, dry_run, reviewed_resources)
            .await
    }

    pub async fn get_session(
        &self,
        session_id: &str,
        with_stats: bool,
    ) -> Result<SessionSummary, AgentHostError> {
        let record = self.get_record(session_id).await?;
        let runtime_session = record.runtime_session.clone();
        let runtime_summary = self.inner.runtime.summary(&runtime_session).await?;
        let stats = if with_stats {
            Some(self.inner.runtime.stats(&runtime_session).await?)
        } else {
            None
        };
        let summary = {
            let mut sessions = self.inner.sessions.write().await;
            let record = sessions
                .get_mut(session_id)
                .ok_or_else(|| AgentHostError::session_not_found(session_id))?;
            record.apply_runtime_summary(runtime_summary);
            if let Some(stats) = stats {
                record.stats = stats;
            }
            let summary = record.summary();
            drop(sessions);
            summary
        };
        Ok(summary)
    }

    pub async fn list_sessions(
        &self,
        with_stats: bool,
    ) -> Result<Vec<SessionSummary>, AgentHostError> {
        let session_ids = self
            .inner
            .sessions
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut summaries = Vec::with_capacity(session_ids.len());
        for session_id in session_ids {
            match self.get_session(&session_id, with_stats).await {
                Ok(summary) => summaries.push(summary),
                Err(error) => {
                    tracing::warn!(%session_id, %error, "failed to refresh session summary");
                    if let Some(record) = self.inner.sessions.read().await.get(&session_id) {
                        summaries.push(record.summary());
                    }
                }
            }
        }
        Ok(summaries)
    }

    pub async fn history(&self, session_id: &str) -> Result<Vec<Vec<KernelEvent>>, AgentHostError> {
        let record = self.get_record(session_id).await?;
        let history = self.inner.runtime.history(&record.runtime_session).await?;
        if let Some(record) = self.inner.sessions.write().await.get_mut(session_id) {
            record.history.clone_from(&history);
        }
        Ok(history)
    }

    pub async fn logs(&self, session_id: &str) -> Result<Vec<String>, AgentHostError> {
        let record = self.get_record(session_id).await?;
        self.inner.runtime.logs(&record.runtime_session).await
    }

    pub async fn container_logs(
        &self,
        session_id: &str,
        tail: Option<u32>,
    ) -> Result<Vec<String>, AgentHostError> {
        let record = self.get_record(session_id).await?;
        self.inner
            .runtime
            .container_logs(&record.runtime_session, tail)
            .await
    }

    pub async fn snapshot_session_workspace(
        &self,
        session_id: &str,
        request: SnapshotWorkspaceRequest,
    ) -> Result<serde_json::Value, AgentHostError> {
        validate_workspace_id_or_error(&request.workspace_id)?;
        validate_volume_name_or_error(&request.volume_name)?;
        let exclude_paths = validate_relative_exclude_paths(request.exclude_paths)?;
        let record = self.get_record(session_id).await?;
        self.inner
            .runtime
            .snapshot_session_workspace(
                &record.runtime_session,
                request.workspace_id,
                request.volume_name,
                exclude_paths,
            )
            .await
    }

    pub async fn clone_workspace(
        &self,
        request: CloneWorkspaceRequest,
    ) -> Result<serde_json::Value, AgentHostError> {
        validate_volume_name_or_error(&request.source_volume_name)?;
        validate_workspace_id_or_error(&request.target_workspace_id)?;
        validate_volume_name_or_error(&request.target_volume_name)?;
        self.inner
            .runtime
            .clone_workspace(
                request.source_volume_name,
                request.target_workspace_id,
                request.target_volume_name,
            )
            .await
    }

    pub async fn open_workspace_vscode(
        &self,
        request: OpenWorkspaceVscodeRequest,
    ) -> Result<serde_json::Value, AgentHostError> {
        validate_workspace_id_or_error(&request.workspace_id)?;
        validate_volume_name_or_error(&request.volume_name)?;
        self.inner
            .runtime
            .open_workspace_vscode(request.workspace_id, request.volume_name)
            .await
    }

    pub async fn telemetry(&self, session_id: &str) -> Result<TelemetrySnapshot, AgentHostError> {
        let record = self.get_record(session_id).await?;
        self.inner.runtime.telemetry(&record.runtime_session).await
    }

    pub async fn terminal_status(
        &self,
        session_id: &str,
    ) -> Result<TerminalStatus, AgentHostError> {
        let record = self.get_terminal_record(session_id).await?;
        self.inner
            .terminal
            .status(session_id, &record.runtime_session)
            .await
    }

    pub async fn terminal_ensure(
        &self,
        session_id: &str,
    ) -> Result<TerminalStatus, AgentHostError> {
        let record = self.get_terminal_record(session_id).await?;
        self.inner
            .terminal
            .ensure(session_id, &record.runtime_session)
            .await
    }

    pub async fn terminal_stop(&self, session_id: &str) -> Result<TerminalStatus, AgentHostError> {
        let record = self.get_terminal_record(session_id).await?;
        self.inner
            .terminal
            .stop(session_id, &record.runtime_session)
            .await
    }

    pub async fn terminal_resume(
        &self,
        session_id: &str,
    ) -> Result<TerminalStatus, AgentHostError> {
        let record = self.get_terminal_record(session_id).await?;
        self.inner
            .terminal
            .resume(session_id, &record.runtime_session)
            .await
    }

    pub async fn terminal_attach(
        &self,
        session_id: &str,
    ) -> Result<TerminalConnection, AgentHostError> {
        let record = self.get_terminal_record(session_id).await?;
        self.inner
            .terminal
            .attach(session_id, &record.runtime_session)
            .await
    }

    pub async fn terminal_resize(
        &self,
        session_id: &str,
        attachment_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), AgentHostError> {
        let record = self.get_terminal_record(session_id).await?;
        self.inner
            .terminal
            .resize(
                session_id,
                &record.runtime_session,
                attachment_id,
                cols,
                rows,
            )
            .await
    }

    pub async fn terminal_detach(
        &self,
        session_id: &str,
        attachment_id: &str,
    ) -> Result<(), AgentHostError> {
        let record = self.get_terminal_record(session_id).await?;
        self.inner
            .terminal
            .detach(session_id, &record.runtime_session, attachment_id)
            .await
    }

    async fn get_record(&self, session_id: &str) -> Result<SessionRecord, AgentHostError> {
        self.inner
            .sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| AgentHostError::session_not_found(session_id))
    }

    async fn get_terminal_record(&self, session_id: &str) -> Result<SessionRecord, AgentHostError> {
        let record = self.get_record(session_id).await?;
        if record.interaction_mode != InteractionMode::Cli {
            return Err(AgentHostError::conflict(
                "terminal routes require a CLI interaction-mode session",
            ));
        }
        Ok(record)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub telemetry_volume_identity: Option<String>,
    #[serde(default)]
    pub harness: HarnessName,
    #[serde(default)]
    pub interaction_mode: InteractionMode,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub additional_paths: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub workspace_mounts: Vec<WorkspaceMountRequest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct WorkspaceMountRequest {
    pub workspace_id: String,
    #[serde(default = "default_workspace_mount_mode")]
    pub mode: String,
    #[serde(default)]
    pub volume_name: Option<String>,
}

impl WorkspaceMountRequest {
    fn into_mount(self) -> Result<WorkspaceMount, AgentHostError> {
        validate_workspace_id_or_error(&self.workspace_id)?;
        let mode = parse_workspace_mount_mode(&self.mode)?;
        if let Some(volume_name) = &self.volume_name {
            validate_volume_name_or_error(volume_name)?;
        }
        Ok(WorkspaceMount {
            workspace_id: self.workspace_id,
            mode,
            volume_name: self.volume_name,
        })
    }
}

impl From<WorkspaceMount> for WorkspaceMountRequest {
    fn from(mount: WorkspaceMount) -> Self {
        Self {
            workspace_id: mount.workspace_id,
            mode: mount.mode.to_string(),
            volume_name: mount.volume_name,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct SendMessageRequest {
    message: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SnapshotWorkspaceRequest {
    pub workspace_id: String,
    pub volume_name: String,
    #[serde(default, alias = "exclude_names")]
    pub exclude_paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CloneWorkspaceRequest {
    pub source_volume_name: String,
    pub target_workspace_id: String,
    pub target_volume_name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OpenWorkspaceVscodeRequest {
    pub workspace_id: String,
    pub volume_name: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct StatsQuery {
    #[serde(default)]
    with_stats: bool,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct ContainerLogsQuery {
    tail: Option<u32>,
    #[serde(default, rename = "all")]
    all_logs: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct CleanupRequest {
    #[serde(default)]
    owned_session_ids: BTreeSet<String>,
    #[serde(default = "default_true")]
    dry_run: bool,
    reviewed_resources: Option<Vec<CleanupResourceIdentity>>,
}

#[derive(Clone)]
struct SessionRecord {
    session_id: String,
    telemetry_volume_identity: Option<String>,
    harness: HarnessName,
    interaction_mode: InteractionMode,
    runtime_session: KernelRuntimeSession,
    env: BTreeMap<String, String>,
    additional_paths: Vec<String>,
    skills: Vec<String>,
    workspace_mounts: Vec<WorkspaceMount>,
    history: Vec<Vec<KernelEvent>>,
    status: KernelStatus,
    resume_token: Option<String>,
    container_name: Option<String>,
    vscode_url: Option<String>,
    free_port_url: Option<String>,
    stats: Option<DockerStatsSummary>,
}

impl SessionRecord {
    fn summary(&self) -> SessionSummary {
        SessionSummary {
            session_id: self.session_id.clone(),
            harness: self.harness,
            interaction_mode: self.interaction_mode,
            status: self.status,
            turns: self.history.len(),
            resume_token: self.resume_token.clone(),
            additional_paths: self.additional_paths.clone(),
            workspace_mounts: self
                .workspace_mounts
                .iter()
                .map(WorkspaceMount::summary)
                .collect(),
            container_name: self.container_name.clone(),
            vscode_url: self.vscode_url.clone(),
            free_port_url: self.free_port_url.clone(),
            stats: self.stats.clone(),
        }
    }

    fn apply_runtime_summary(&mut self, summary: RuntimeSessionSummary) {
        if let Some(resume_token) = non_empty(summary.resume_token) {
            self.resume_token = Some(resume_token);
        }
        if let Some(status) = summary.status {
            self.status = status;
        }
        if let Some(vscode_url) = non_empty(summary.vscode_url) {
            self.vscode_url = Some(vscode_url);
        }
        if let Some(free_port_url) = non_empty(summary.free_port_url) {
            self.free_port_url = Some(free_port_url);
        }
    }
}

struct AgentEventStream {
    inner: Option<EventStream>,
    runtime: Arc<dyn KernelRuntime>,
    sessions: Arc<RwLock<BTreeMap<String, SessionRecord>>>,
    session_id: String,
    runtime_session: KernelRuntimeSession,
    events: Vec<KernelEvent>,
    finalize: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    finalized: bool,
}

impl AgentEventStream {
    fn new(
        inner: EventStream,
        runtime: Arc<dyn KernelRuntime>,
        sessions: Arc<RwLock<BTreeMap<String, SessionRecord>>>,
        session_id: String,
        runtime_session: KernelRuntimeSession,
    ) -> Self {
        Self {
            inner: Some(inner),
            runtime,
            sessions,
            session_id,
            runtime_session,
            events: Vec::new(),
            finalize: None,
            finalized: false,
        }
    }

    fn begin_finalize(&mut self) {
        if self.finalized || self.finalize.is_some() {
            return;
        }
        self.inner.take();
        let runtime = self.runtime.clone();
        let sessions = self.sessions.clone();
        let session_id = self.session_id.clone();
        let runtime_session = self.runtime_session.clone();
        let events = std::mem::take(&mut self.events);
        self.finalize = Some(Box::pin(finalize_stream(
            runtime,
            sessions,
            session_id,
            runtime_session,
            events,
        )));
    }
}

impl Stream for AgentEventStream {
    type Item = Result<KernelEvent, AgentHostError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(finalize) = self.finalize.as_mut() {
                match finalize.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        self.finalize = None;
                        self.finalized = true;
                        return Poll::Ready(None);
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            let Some(inner) = self.inner.as_mut() else {
                self.begin_finalize();
                continue;
            };

            match inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    self.events.push(event.clone());
                    return Poll::Ready(Some(Ok(event)));
                }
                Poll::Ready(Some(Err(error))) => {
                    self.begin_finalize();
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    self.begin_finalize();
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl Drop for AgentEventStream {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        self.inner.take();
        if let Some(finalize) = self.finalize.take() {
            tokio::spawn(finalize);
            return;
        }
        let runtime = self.runtime.clone();
        let sessions = self.sessions.clone();
        let session_id = self.session_id.clone();
        let runtime_session = self.runtime_session.clone();
        let events = std::mem::take(&mut self.events);
        tokio::spawn(finalize_stream(
            runtime,
            sessions,
            session_id,
            runtime_session,
            events,
        ));
    }
}

fn validate_existing_session_identity(
    record: &SessionRecord,
    request: &CreateSessionRequest,
) -> Result<(), AgentHostError> {
    if record.harness != request.harness || record.interaction_mode != request.interaction_mode {
        return Err(AgentHostError::conflict(format!(
            "session {:?} is already registered as harness {:?} in {:?} mode",
            record.session_id, record.harness, record.interaction_mode
        )));
    }
    if record.telemetry_volume_identity != request.telemetry_volume_identity {
        return Err(AgentHostError::conflict(format!(
            "session {:?} is already registered with telemetry volume identity {:?}",
            record.session_id, record.telemetry_volume_identity
        )));
    }
    Ok(())
}

async fn finalize_stream(
    runtime: Arc<dyn KernelRuntime>,
    sessions: Arc<RwLock<BTreeMap<String, SessionRecord>>>,
    session_id: String,
    runtime_session: KernelRuntimeSession,
    events: Vec<KernelEvent>,
) {
    let runtime_summary = runtime.summary(&runtime_session).await;
    {
        let mut sessions = sessions.write().await;
        let Some(record) = sessions.get_mut(&session_id) else {
            return;
        };
        if !events.is_empty() {
            record.status = derive_status(&events, record.status);
            record.history.push(events);
        }
        match runtime_summary {
            Ok(summary) => record.apply_runtime_summary(summary),
            Err(error) => tracing::warn!(%session_id, %error, "failed to refresh session summary"),
        }
        drop(sessions);
    }
}

fn derive_status(events: &[KernelEvent], fallback_status: KernelStatus) -> KernelStatus {
    events
        .iter()
        .rev()
        .find_map(|event| {
            (event.event_type == KernelEventType::SessionStatus)
                .then_some(event.status)
                .flatten()
        })
        .unwrap_or(fallback_status)
}

fn default_workspace_mount_mode() -> String {
    "rw".to_owned()
}

fn validate_workspace_mount_requests(
    mounts: Vec<WorkspaceMountRequest>,
) -> Result<Vec<WorkspaceMount>, AgentHostError> {
    let mut seen = BTreeSet::new();
    let mut validated = Vec::with_capacity(mounts.len());
    for request in mounts {
        let mount = request.into_mount()?;
        if !seen.insert(mount.workspace_id.clone()) {
            return Err(AgentHostError::validation(format!(
                "workspace {:?} is mounted more than once",
                mount.workspace_id
            )));
        }
        validated.push(mount);
    }
    Ok(validated)
}

fn parse_workspace_mount_mode(mode: &str) -> Result<WorkspaceMountMode, AgentHostError> {
    match mode {
        "ro" => Ok(WorkspaceMountMode::ReadOnly),
        "rw" => Ok(WorkspaceMountMode::ReadWrite),
        _ => Err(AgentHostError::validation(
            "mode must be either 'rw' or 'ro'",
        )),
    }
}

fn append_unique_paths(mut base_paths: Vec<String>, extra_paths: Vec<String>) -> Vec<String> {
    let mut seen = base_paths.iter().cloned().collect::<BTreeSet<_>>();
    for path in extra_paths {
        if seen.insert(path.clone()) {
            base_paths.push(path);
        }
    }
    base_paths
}

fn validate_workspace_id_or_error(workspace_id: &str) -> Result<(), AgentHostError> {
    if valid_workspace_id(workspace_id) {
        Ok(())
    } else {
        Err(AgentHostError::validation(
            "workspace_id must use lowercase letters, digits, and single dashes only",
        ))
    }
}

fn validate_volume_name_or_error(volume_name: &str) -> Result<(), AgentHostError> {
    if valid_volume_name(volume_name) {
        Ok(())
    } else {
        Err(AgentHostError::validation(
            "volume_name must start with an alphanumeric character and contain only letters, digits, underscore, dot, or dash",
        ))
    }
}

fn validate_relative_exclude_paths(paths: Vec<String>) -> Result<Vec<String>, AgentHostError> {
    let mut validated = Vec::with_capacity(paths.len());
    let mut seen = BTreeSet::new();
    for path in paths {
        let parsed = FsPath::new(&path);
        let valid = !path.is_empty()
            && !path.contains('\\')
            && !path.contains('\0')
            && !parsed.is_absolute()
            && parsed
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
            && path.split('/').all(|component| !component.is_empty());
        if !valid {
            return Err(AgentHostError::validation(format!(
                "exclude_paths entries must be normalized relative paths without traversal: {path:?}"
            )));
        }
        if seen.insert(path.clone()) {
            validated.push(path);
        }
    }
    Ok(validated)
}

fn valid_workspace_id(workspace_id: &str) -> bool {
    !workspace_id.is_empty()
        && workspace_id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn valid_volume_name(volume_name: &str) -> bool {
    let mut bytes = volume_name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn validate_session_id_or_error(session_id: &str) -> Result<(), AgentHostError> {
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AgentHostError::validation(
            "session_id must be 1-128 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(())
}

const fn default_true() -> bool {
    true
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sessions", post(create_session).get(list_sessions))
        .route(
            "/sessions/{session_id}",
            get(get_session).delete(destroy_session),
        )
        .route("/sessions/{session_id}/messages", post(send_message))
        .route(
            "/sessions/{session_id}/messages/stream",
            post(stream_message),
        )
        .route("/sessions/{session_id}/history", get(history))
        .route("/sessions/{session_id}/logs", get(session_logs))
        .route("/sessions/{session_id}/telemetry", get(session_telemetry))
        .route(
            "/sessions/{session_id}/container-logs",
            get(session_container_logs),
        )
        .route("/sessions/{session_id}/reset", post(reset_session))
        .route(
            "/sessions/{session_id}/workspace/snapshot",
            post(snapshot_session_workspace),
        )
        .route("/workspaces/clone", post(clone_workspace))
        .route("/workspaces/vscode", post(open_workspace_vscode))
        .route("/management/runtime-cleanup", post(cleanup_orphans))
}

async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<SessionSummary>, ApiError> {
    state
        .sessions
        .create_session(payload)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<Vec<SessionSummary>>, ApiError> {
    state
        .sessions
        .list_sessions(query.with_stats)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<SessionSummary>, ApiError> {
    state
        .sessions
        .get_session(&session_id, query.with_stats)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn send_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let events = state
        .sessions
        .send_message(&session_id, payload.message)
        .await?;
    Ok(Json(json!({ "events": events })))
}

async fn stream_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<Response, ApiError> {
    let stream = state
        .sessions
        .stream_message(&session_id, payload.message)
        .await?;
    let lines = stream.map(|event| {
        event.and_then(|event| {
            let mut line = event.to_jsonl()?;
            line.push('\n');
            Ok(Bytes::from(line))
        })
    });
    let mut response = Body::from_stream(lines).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/x-ndjson"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        header::HeaderValue::from_static("no"),
    );
    Ok(response)
}

async fn history(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let turns = state.sessions.history(&session_id).await?;
    Ok(Json(json!({ "history": turns })))
}

async fn session_logs(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let lines = state.sessions.logs(&session_id).await?;
    Ok(Json(json!({ "lines": lines })))
}

async fn session_telemetry(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<TelemetrySnapshot>, ApiError> {
    state
        .sessions
        .telemetry(&session_id)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn session_container_logs(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ContainerLogsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tail = if query.all_logs {
        None
    } else {
        Some(query.tail.unwrap_or(2_000))
    };
    if let Some(tail) = tail
        && !(1..=50_000).contains(&tail)
    {
        return Err(ApiError(AgentHostError::validation(
            "tail must be between 1 and 50000",
        )));
    }
    let lines = state.sessions.container_logs(&session_id, tail).await?;
    Ok(Json(json!({ "lines": lines })))
}

async fn reset_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionSummary>, ApiError> {
    state
        .sessions
        .reset_session(&session_id)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn snapshot_session_workspace(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(payload): Json<SnapshotWorkspaceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .sessions
        .snapshot_session_workspace(&session_id, payload)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn clone_workspace(
    State(state): State<AppState>,
    Json(payload): Json<CloneWorkspaceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .sessions
        .clone_workspace(payload)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn open_workspace_vscode(
    State(state): State<AppState>,
    Json(payload): Json<OpenWorkspaceVscodeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .sessions
        .open_workspace_vscode(payload)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn cleanup_orphans(
    State(state): State<AppState>,
    Json(payload): Json<CleanupRequest>,
) -> Result<Json<CleanupReport>, ApiError> {
    state
        .sessions
        .cleanup_orphans(
            &payload.owned_session_ids,
            payload.dry_run,
            payload.reviewed_resources.as_deref(),
        )
        .await
        .map(Json)
        .map_err(ApiError::from)
}

async fn destroy_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.sessions.destroy_session(&session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) struct ApiError(pub(crate) AgentHostError);

impl From<AgentHostError> for ApiError {
    fn from(error: AgentHostError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            AgentHostError::SessionNotFound { .. }
            | AgentHostError::TerminalAttachmentNotFound { .. } => StatusCode::NOT_FOUND,
            AgentHostError::Validation { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            AgentHostError::Conflict { .. } => StatusCode::CONFLICT,
            AgentHostError::UpstreamUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            AgentHostError::Runtime { .. }
            | AgentHostError::Docker { .. }
            | AgentHostError::Http { .. }
            | AgentHostError::Io { .. }
            | AgentHostError::Json { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let detail = self.0.to_string();
        (status, Json(json!({ "detail": detail }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::{Path, PathBuf},
        sync::{Arc, Mutex, MutexGuard, PoisonError},
    };

    use async_trait::async_trait;
    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode, header},
    };
    use futures_util::{StreamExt, stream};
    use http_body_util::BodyExt;
    use serde_json::{Value as JsonValue, json};
    use tower::ServiceExt;

    use super::{
        CreateSessionRequest, EventStream, KernelRuntime, RuntimeCreateSession, SessionRegistry,
    };
    use crate::{
        AppConfig, AppState, build_router,
        errors::AgentHostError,
        models::{
            ActivityCounts, CacheReportingState, CacheSignal, CacheSignalConfidence,
            CacheSignalReason, CacheSignalState, CleanupReport, CleanupResourceIdentity,
            ContextUsage, DockerStatsSummary, HarnessName, InteractionMode, KernelEvent,
            KernelEventType, KernelRuntimeSession, KernelStatus, ModelCallSummary,
            ReportingCoverage, RuntimeSessionSummary, SubagentBreakdown, TelemetryContentMode,
            TelemetrySnapshot, TelemetryState, TelemetryWarning, TelemetryWarningCode,
            TelemetryWarningSummary, TokenAccountingConvention, UsageBreakdown, WorkspaceMount,
            WorkspaceMountMode,
        },
        skills::{SkillRegistry, SkillVolumeMode, SkillVolumeResource, SkillsService},
    };

    #[derive(Clone, Default)]
    struct FakeRuntime {
        state: Arc<Mutex<FakeState>>,
    }

    #[derive(Default)]
    struct FakeState {
        created: Vec<RuntimeCreateSession>,
        destroyed: Vec<String>,
        sent: Vec<(String, String)>,
        summaries: BTreeMap<String, RuntimeSessionSummary>,
        histories: BTreeMap<String, Vec<Vec<KernelEvent>>>,
        telemetry: BTreeMap<String, TelemetrySnapshot>,
        fail_summary: bool,
        fail_telemetry: bool,
    }

    impl FakeRuntime {
        fn state(&self) -> MutexGuard<'_, FakeState> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    #[async_trait]
    impl KernelRuntime for FakeRuntime {
        async fn create_session(
            &self,
            request: RuntimeCreateSession,
        ) -> Result<KernelRuntimeSession, AgentHostError> {
            let container_name = format!("container-{}", &request.session_id[..8]);
            let harness = request.harness;
            {
                let mut state = self.state();
                state.created.push(request);
                state.summaries.insert(
                    container_name.clone(),
                    RuntimeSessionSummary {
                        status: Some(KernelStatus::Idle),
                        resume_token: Some("resume-runtime-1".to_owned()),
                        vscode_url: None,
                        free_port_url: None,
                    },
                );
                state.histories.insert(container_name.clone(), Vec::new());
                state.telemetry.insert(
                    container_name.clone(),
                    telemetry_snapshot_for_harness(harness),
                );
            }
            Ok(KernelRuntimeSession::opaque(container_name))
        }

        fn stream_message(
            &self,
            session: KernelRuntimeSession,
            message: String,
        ) -> Result<EventStream, AgentHostError> {
            let container_name = session_key(&session);
            let events = vec![
                session_start("kernel-session", "stub"),
                status_event(KernelStatus::Busy),
                text_delta(&message),
                status_event(KernelStatus::Done),
                session_end(),
            ];
            {
                let mut state = self.state();
                state.sent.push((container_name.clone(), message));
                state
                    .histories
                    .entry(container_name.clone())
                    .or_default()
                    .push(events.clone());
                state.summaries.insert(
                    container_name,
                    RuntimeSessionSummary {
                        status: Some(KernelStatus::Done),
                        resume_token: Some("resume-runtime-2".to_owned()),
                        vscode_url: None,
                        free_port_url: None,
                    },
                );
            }
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
        }

        async fn summary(
            &self,
            session: &KernelRuntimeSession,
        ) -> Result<RuntimeSessionSummary, AgentHostError> {
            let key = session_key(session);
            {
                let state = self.state();
                if state.fail_summary {
                    return Err(AgentHostError::runtime("kernel unreachable"));
                }
                state.summaries.get(&key).cloned()
            }
            .ok_or_else(|| AgentHostError::runtime("missing summary"))
        }

        async fn history(
            &self,
            session: &KernelRuntimeSession,
        ) -> Result<Vec<Vec<KernelEvent>>, AgentHostError> {
            let key = session_key(session);
            Ok(self
                .state()
                .histories
                .get(&key)
                .cloned()
                .unwrap_or_default())
        }

        async fn logs(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<Vec<String>, AgentHostError> {
            Ok(vec![r#"{"type":"stub","data":{}}"#.to_owned()])
        }

        async fn container_logs(
            &self,
            session: &KernelRuntimeSession,
            tail: Option<u32>,
        ) -> Result<Vec<String>, AgentHostError> {
            let key = session_key(session);
            let lines = (0..5)
                .map(|index| format!("{key} container line {index}"))
                .collect::<Vec<_>>();
            let Some(tail) = tail else {
                return Ok(lines);
            };
            let split_at = lines.len().saturating_sub(tail as usize);
            Ok(lines[split_at..].to_vec())
        }

        async fn stats(
            &self,
            _session: &KernelRuntimeSession,
        ) -> Result<Option<DockerStatsSummary>, AgentHostError> {
            Ok(Some(DockerStatsSummary {
                cpu_percent: Some(12.5),
                memory_usage_bytes: Some(50_000_000),
                memory_limit_bytes: Some(200_000_000),
                memory_percent: Some(25.0),
            }))
        }

        fn container_name(&self, session: &KernelRuntimeSession) -> Option<String> {
            Some(session_key(session))
        }

        fn vscode_url(&self, session: &KernelRuntimeSession) -> Option<String> {
            let key = session_key(session);
            Some(format!("http://127.0.0.1/vscode/{key}"))
        }

        fn free_port_url(&self, session: &KernelRuntimeSession) -> Option<String> {
            let key = session_key(session);
            Some(format!("http://127.0.0.1/free/{key}"))
        }

        async fn destroy_session(
            &self,
            session: KernelRuntimeSession,
        ) -> Result<(), AgentHostError> {
            {
                self.state().destroyed.push(session_key(&session));
            }
            Ok(())
        }

        async fn destroy_session_by_id(&self, session_id: &str) -> Result<(), AgentHostError> {
            self.state().destroyed.push(session_id.to_owned());
            Ok(())
        }

        async fn cleanup_orphans(
            &self,
            owned_session_ids: &BTreeSet<String>,
            dry_run: bool,
            _reviewed_resources: Option<&[CleanupResourceIdentity]>,
        ) -> Result<CleanupReport, AgentHostError> {
            Ok(CleanupReport {
                dry_run,
                owned_session_count: owned_session_ids.len(),
                resources: Vec::new(),
                deleted_count: 0,
                error_count: 0,
            })
        }

        async fn snapshot_session_workspace(
            &self,
            session: &KernelRuntimeSession,
            workspace_id: String,
            volume_name: String,
            exclude_paths: Vec<String>,
        ) -> Result<serde_json::Value, AgentHostError> {
            Ok(json!({
                "session": session_key(session),
                "workspace_id": workspace_id,
                "volume_name": volume_name,
                "exclude_paths": exclude_paths,
            }))
        }

        async fn clone_workspace(
            &self,
            source_volume_name: String,
            target_workspace_id: String,
            target_volume_name: String,
        ) -> Result<serde_json::Value, AgentHostError> {
            Ok(json!({
                "source_volume_name": source_volume_name,
                "workspace_id": target_workspace_id,
                "volume_name": target_volume_name,
            }))
        }

        async fn open_workspace_vscode(
            &self,
            workspace_id: String,
            volume_name: String,
        ) -> Result<serde_json::Value, AgentHostError> {
            Ok(json!({
                "workspace_id": workspace_id,
                "volume_name": volume_name,
                "container_name": format!("editor-{workspace_id}"),
                "vscode_url": "http://127.0.0.1:12345",
            }))
        }

        async fn telemetry(
            &self,
            session: &KernelRuntimeSession,
        ) -> Result<TelemetrySnapshot, AgentHostError> {
            let key = session_key(session);
            {
                let state = self.state();
                if state.fail_telemetry {
                    return Err(AgentHostError::upstream_unavailable(
                        "kernel telemetry provider is unavailable",
                    ));
                }
                state.telemetry.get(&key).cloned()
            }
            .ok_or_else(|| AgentHostError::runtime("missing telemetry"))
        }
    }

    #[allow(clippy::too_many_lines)]
    fn telemetry_snapshot_for_harness(harness: HarnessName) -> TelemetrySnapshot {
        if harness != HarnessName::CopilotCli {
            return TelemetrySnapshot::unavailable(format!(
                "telemetry is unavailable for harness {}",
                harness.as_str()
            ));
        }

        TelemetrySnapshot {
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
            subagents: SubagentBreakdown {
                invocations: 1,
                model_calls: 2,
                effective_input_tokens: Some(3),
                output_tokens: Some(4),
                cache_read_input_tokens: Some(5),
                cache_write_input_tokens: Some(6),
                duration_ms: Some(7),
            },
            cache_signal: Some(CacheSignal {
                state: CacheSignalState::CacheResetSuspected,
                confidence: Some(CacheSignalConfidence::Medium),
                reason: Some(CacheSignalReason::ContextDiscontinuity),
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
        }
    }

    #[tokio::test]
    async fn create_send_history_and_destroy() {
        let runtime = FakeRuntime::default();
        let registry = SessionRegistry::with_runtime(Arc::new(runtime.clone()));
        let mut env = BTreeMap::new();
        env.insert("COPILOT_MODEL".to_owned(), "gpt-5.2".to_owned());

        let session = registry
            .create_session(CreateSessionRequest {
                session_id: None,
                telemetry_volume_identity: None,
                harness: HarnessName::CopilotCli,
                interaction_mode: InteractionMode::Chat,
                env,
                additional_paths: vec!["/srv/agent".to_owned()],
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("create failed: {error}"));
        let events = registry
            .send_message(&session.session_id, "hello".to_owned())
            .await
            .unwrap_or_else(|error| panic!("send failed: {error}"));
        let history = registry
            .history(&session.session_id)
            .await
            .unwrap_or_else(|error| panic!("history failed: {error}"));
        let fetched = registry
            .get_session(&session.session_id, false)
            .await
            .unwrap_or_else(|error| panic!("get failed: {error}"));

        {
            let state = runtime.state();
            assert_eq!(state.created[0].harness, HarnessName::CopilotCli);
            assert_eq!(state.created[0].additional_paths, vec!["/srv/agent"]);
            assert_eq!(state.created[0].env["COPILOT_MODEL"], "gpt-5.2");
            assert_eq!(state.sent[0].1, "hello");
            drop(state);
        }
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                "session/start",
                "session/status",
                "text_delta",
                "session/status",
                "session/end"
            ]
        );
        assert_eq!(history.len(), 1);
        assert_eq!(fetched.turns, 1);
        assert_eq!(fetched.resume_token.as_deref(), Some("resume-runtime-2"));

        registry
            .destroy_session(&session.session_id)
            .await
            .unwrap_or_else(|error| panic!("destroy failed: {error}"));
        assert_eq!(runtime.state().destroyed.len(), 1);
    }

    #[tokio::test]
    async fn requested_session_identity_is_validated_reensured_and_immutable() {
        let runtime = FakeRuntime::default();
        let registry = SessionRegistry::with_runtime(Arc::new(runtime.clone()));
        let request = CreateSessionRequest {
            session_id: Some("stable-session-123".to_owned()),
            telemetry_volume_identity: Some("stable-session-123".to_owned()),
            harness: HarnessName::Echo,
            interaction_mode: InteractionMode::Chat,
            env: BTreeMap::new(),
            additional_paths: Vec::new(),
            skills: Vec::new(),
            workspace_mounts: Vec::new(),
        };

        let first = registry
            .create_session(request.clone())
            .await
            .unwrap_or_else(|error| panic!("first create failed: {error}"));
        let second = registry
            .create_session(request)
            .await
            .unwrap_or_else(|error| panic!("idempotent create failed: {error}"));

        assert_eq!(first.session_id, "stable-session-123");
        assert_eq!(second.session_id, first.session_id);
        assert_eq!(first.interaction_mode, InteractionMode::Chat);
        {
            let state = runtime.state();
            assert_eq!(state.created.len(), 2);
            assert_eq!(
                state.created[0].telemetry_volume_identity.as_deref(),
                Some("stable-session-123")
            );
            drop(state);
        }

        let result = registry
            .create_session(CreateSessionRequest {
                session_id: Some(first.session_id),
                telemetry_volume_identity: Some("stable-session-123".to_owned()),
                harness: HarnessName::CopilotCli,
                interaction_mode: InteractionMode::Cli,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await;
        let Err(conflict) = result else {
            panic!("registered session identity must not change mode or harness");
        };
        assert!(matches!(conflict, AgentHostError::Conflict { .. }));
        assert_eq!(runtime.state().created.len(), 2);

        let result = registry
            .create_session(CreateSessionRequest {
                session_id: Some("stable-session-123".to_owned()),
                telemetry_volume_identity: Some("different-telemetry".to_owned()),
                harness: HarnessName::Echo,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await;
        let Err(conflict) = result else {
            panic!("registered session identity must not change telemetry identity");
        };
        assert!(matches!(conflict, AgentHostError::Conflict { .. }));
        assert_eq!(runtime.state().created.len(), 2);

        let result = registry
            .create_session(CreateSessionRequest {
                session_id: Some("not valid!".to_owned()),
                telemetry_volume_identity: None,
                harness: HarnessName::Echo,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await;
        let Err(invalid) = result else {
            panic!("invalid identity should fail");
        };
        assert!(matches!(invalid, AgentHostError::Validation { .. }));

        let result = registry
            .create_session(CreateSessionRequest {
                session_id: Some("valid-but-not-uuid".to_owned()),
                telemetry_volume_identity: None,
                harness: HarnessName::CopilotCli,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await;
        let Err(invalid) = result else {
            panic!("non-UUID Copilot identity should fail");
        };
        assert!(matches!(invalid, AgentHostError::Validation { .. }));
    }

    #[tokio::test]
    async fn forgetting_sessions_for_shutdown_is_non_destructive() {
        let runtime = FakeRuntime::default();
        let registry = SessionRegistry::with_runtime(Arc::new(runtime.clone()));
        registry
            .create_session(CreateSessionRequest {
                session_id: Some("stable-session".to_owned()),
                telemetry_volume_identity: None,
                harness: HarnessName::Echo,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("create failed: {error}"));

        registry.forget_all_sessions().await;

        assert!(
            registry
                .list_sessions(false)
                .await
                .unwrap_or_default()
                .is_empty()
        );
        assert!(runtime.state().destroyed.is_empty());
    }

    #[tokio::test]
    async fn create_session_records_workspace_mounts() {
        let runtime = FakeRuntime::default();
        let registry = SessionRegistry::with_runtime(Arc::new(runtime.clone()));

        let session = registry
            .create_session(CreateSessionRequest {
                session_id: None,
                telemetry_volume_identity: None,
                harness: HarnessName::CopilotCli,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: vec![
                    WorkspaceMount::new("todo-list-code", WorkspaceMountMode::ReadWrite).into(),
                    WorkspaceMount::new("todo-list-items", WorkspaceMountMode::ReadOnly).into(),
                ],
            })
            .await
            .unwrap_or_else(|error| panic!("create failed: {error}"));

        {
            let state = runtime.state();
            assert_eq!(
                state.created[0].additional_paths,
                vec!["/workspace/todo-list-code", "/workspace/todo-list-items"]
            );
            drop(state);
        }
        let mounts = serde_json::to_value(session.workspace_mounts)
            .unwrap_or_else(|error| panic!("serialize mounts failed: {error}"));
        assert_eq!(
            mounts,
            json!([
                {
                    "workspace_id": "todo-list-code",
                    "mode": "rw",
                    "mount_path": "/workspace/todo-list-code",
                    "volume_name": "agentspace-workspace-todo-list-code"
                },
                {
                    "workspace_id": "todo-list-items",
                    "mode": "ro",
                    "mount_path": "/workspace/todo-list-items",
                    "volume_name": "agentspace-workspace-todo-list-items"
                }
            ])
        );
    }

    #[tokio::test]
    async fn skill_volumes_are_opt_in_and_only_advertised_paths_are_exposed() {
        let root = session_skill_test_dir("volume-privacy");
        let skills = volume_privacy_test_skills(&root);
        let runtime = FakeRuntime::default();
        let registry = SessionRegistry::with_runtime_and_skills(Arc::new(runtime.clone()), skills);

        let enabled = registry
            .create_session(CreateSessionRequest {
                session_id: None,
                telemetry_volume_identity: None,
                harness: HarnessName::CopilotCli,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: vec!["/srv/original".to_owned()],
                skills: vec!["memory".to_owned(), "published".to_owned()],
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("failed to create enabled session: {error}"));
        registry
            .create_session(CreateSessionRequest {
                session_id: None,
                telemetry_volume_identity: None,
                harness: HarnessName::CopilotCli,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("failed to create disabled session: {error}"));

        {
            let state = runtime.state();
            assert_eq!(
                state.created[0].skill_volumes,
                vec![
                    SkillVolumeResource {
                        skill_id: "memory".to_owned(),
                        resource_id: "data".to_owned(),
                        mount_path: "/var/lib/agentspace/memory".to_owned(),
                        advertise: false,
                        mode: SkillVolumeMode::Rw,
                    },
                    SkillVolumeResource {
                        skill_id: "published".to_owned(),
                        resource_id: "docs".to_owned(),
                        mount_path: "/srv/published".to_owned(),
                        advertise: true,
                        mode: SkillVolumeMode::Ro,
                    },
                ]
            );
            assert_eq!(
                state.created[0].additional_paths,
                vec!["/srv/original", "/srv/published"]
            );
            assert!(state.created[1].skill_volumes.is_empty());
            drop(state);
        }
        assert_eq!(
            enabled.additional_paths,
            vec!["/srv/original", "/srv/published"]
        );
        assert!(
            !serde_json::to_string(&enabled)
                .unwrap_or_else(|error| panic!("failed to serialize session: {error}"))
                .contains("/var/lib/agentspace/memory")
        );

        fs::remove_dir_all(&root)
            .unwrap_or_else(|error| panic!("failed to remove {}: {error}", root.display()));
    }

    fn volume_privacy_test_skills(root: &Path) -> SkillRegistry {
        let builtin_dir = root.join("builtin");
        write_test_file(
            &builtin_dir.join("memory/SKILL.md"),
            "# Memory\nUse the memory CLI.",
        );
        write_test_file(
            &builtin_dir.join("memory/agentspace.json"),
            r#"{
                "schema_version": 1,
                "resources": {
                    "volumes": [{
                        "id": "data",
                        "scope": "installation",
                        "mount_path": "/var/lib/agentspace/memory",
                        "advertise": false,
                        "mode": "rw"
                    }]
                }
            }"#,
        );
        write_test_file(&builtin_dir.join("published/SKILL.md"), "# Published");
        write_test_file(
            &builtin_dir.join("published/agentspace.json"),
            r#"{
                "schema_version": 1,
                "resources": {
                    "volumes": [{
                        "id": "docs",
                        "scope": "installation",
                        "mount_path": "/srv/published",
                        "advertise": true,
                        "mode": "ro"
                    }]
                }
            }"#,
        );
        SkillRegistry::from_synced_service(SkillsService::new(root.join("skills"), builtin_dir))
            .unwrap_or_else(|error| panic!("failed to sync test skills: {error}"))
    }

    fn session_skill_test_dir(name: &str) -> PathBuf {
        let path = std::env::current_dir()
            .unwrap_or_else(|error| panic!("failed to read current dir: {error}"))
            .join("target")
            .join("agent_host_rs_session_skill_tests")
            .join(format!("{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", path.display()));
        path
    }

    fn write_test_file(path: &Path, content: &str) {
        fs::create_dir_all(
            path.parent()
                .unwrap_or_else(|| panic!("test path has no parent: {}", path.display())),
        )
        .unwrap_or_else(|error| panic!("failed to create parent for {}: {error}", path.display()));
        fs::write(path, content)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
    }

    #[tokio::test]
    async fn stream_message_updates_history_and_status() {
        let registry = SessionRegistry::with_runtime(Arc::new(FakeRuntime::default()));
        let session = registry
            .create_session(CreateSessionRequest {
                session_id: None,
                telemetry_volume_identity: None,
                harness: HarnessName::Echo,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("create failed: {error}"));

        let events = registry
            .stream_message(&session.session_id, "hello".to_owned())
            .await
            .unwrap_or_else(|error| panic!("stream failed: {error}"))
            .collect::<Vec<_>>()
            .await;
        let fetched = registry
            .get_session(&session.session_id, false)
            .await
            .unwrap_or_else(|error| panic!("get failed: {error}"));

        assert_eq!(events.len(), 5);
        assert_eq!(fetched.turns, 1);
        assert_eq!(fetched.status, KernelStatus::Done);
    }

    #[tokio::test]
    async fn stream_message_finalizes_when_consumer_closes_early() {
        let registry = SessionRegistry::with_runtime(Arc::new(FakeRuntime::default()));
        let session = registry
            .create_session(CreateSessionRequest {
                session_id: None,
                telemetry_volume_identity: None,
                harness: HarnessName::Echo,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("create failed: {error}"));

        let mut stream = registry
            .stream_message(&session.session_id, "hello".to_owned())
            .await
            .unwrap_or_else(|error| panic!("stream failed: {error}"));
        let first = stream
            .next()
            .await
            .unwrap_or_else(|| panic!("stream ended before first event"))
            .unwrap_or_else(|error| panic!("first event failed: {error}"));
        drop(stream);
        tokio::task::yield_now().await;
        let fetched = registry
            .get_session(&session.session_id, false)
            .await
            .unwrap_or_else(|error| panic!("get failed: {error}"));

        assert_eq!(first.event_type, KernelEventType::SessionStart);
        assert_eq!(fetched.turns, 1);
        assert_eq!(fetched.status, KernelStatus::Done);
    }

    #[tokio::test]
    async fn list_sessions_returns_cached_summary_on_failure() {
        let runtime = FakeRuntime::default();
        let registry = SessionRegistry::with_runtime(Arc::new(runtime.clone()));
        let session = registry
            .create_session(CreateSessionRequest {
                session_id: None,
                telemetry_volume_identity: None,
                harness: HarnessName::Echo,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("create failed: {error}"));
        runtime.state().fail_summary = true;

        let summaries = registry
            .list_sessions(false)
            .await
            .unwrap_or_else(|error| panic!("list failed: {error}"));

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, session.session_id);
    }

    #[tokio::test]
    async fn get_session_includes_stats_only_when_requested() {
        let registry = SessionRegistry::with_runtime(Arc::new(FakeRuntime::default()));
        let session = registry
            .create_session(CreateSessionRequest {
                session_id: None,
                telemetry_volume_identity: None,
                harness: HarnessName::Echo,
                interaction_mode: InteractionMode::Chat,
                env: BTreeMap::new(),
                additional_paths: Vec::new(),
                skills: Vec::new(),
                workspace_mounts: Vec::new(),
            })
            .await
            .unwrap_or_else(|error| panic!("create failed: {error}"));

        let without_stats = registry
            .get_session(&session.session_id, false)
            .await
            .unwrap_or_else(|error| panic!("get failed: {error}"));
        let with_stats = registry
            .get_session(&session.session_id, true)
            .await
            .unwrap_or_else(|error| panic!("get failed: {error}"));

        assert!(without_stats.stats.is_none());
        assert_eq!(
            with_stats.stats.and_then(|stats| stats.memory_usage_bytes),
            Some(50_000_000)
        );
    }

    #[tokio::test]
    async fn session_routes_create_message_logs_and_delete() {
        let app = router_with_runtime(FakeRuntime::default());
        let (created_status, created, session_id) = create_mounted_session(&app).await;

        let (listed_status, listed) =
            empty_request(&app, Method::GET, "/sessions?with_stats=true").await;
        let (message_status, message) = json_request(
            &app,
            Method::POST,
            &format!("/sessions/{session_id}/messages"),
            json!({"message": "hello"}),
        )
        .await;
        let (history_status, history) = empty_request(
            &app,
            Method::GET,
            &format!("/sessions/{session_id}/history"),
        )
        .await;
        let (logs_status, logs) =
            empty_request(&app, Method::GET, &format!("/sessions/{session_id}/logs")).await;
        let (telemetry_status, telemetry) = empty_request(
            &app,
            Method::GET,
            &format!("/sessions/{session_id}/telemetry"),
        )
        .await;
        let (container_logs_status, container_logs) = empty_request(
            &app,
            Method::GET,
            &format!("/sessions/{session_id}/container-logs?tail=2"),
        )
        .await;
        let deleted_status =
            status_request(&app, Method::DELETE, &format!("/sessions/{session_id}")).await;
        let after_status =
            status_request(&app, Method::GET, &format!("/sessions/{session_id}")).await;

        assert_eq!(created_status, StatusCode::OK);
        assert_eq!(created["harness"], "copilot-cli");
        assert_eq!(
            created["workspace_mounts"][0]["volume_name"],
            "agentspace-workspace-todo-list-code"
        );
        assert_eq!(listed_status, StatusCode::OK);
        assert_eq!(listed[0]["stats"]["memory_usage_bytes"], 50_000_000);
        assert_eq!(message_status, StatusCode::OK);
        assert_eq!(message["events"][2]["content"], "hello");
        assert_eq!(history_status, StatusCode::OK);
        assert_eq!(history["history"][0][2]["content"], "hello");
        assert_eq!(logs_status, StatusCode::OK);
        assert_eq!(logs["lines"][0], r#"{"type":"stub","data":{}}"#);
        assert_eq!(telemetry_status, StatusCode::OK);
        assert_eq!(telemetry["state"], "live");
        assert_eq!(telemetry["content_mode"], "metadata");
        assert_eq!(telemetry["latest_call"]["cache_reporting"], "reported");
        assert_eq!(
            telemetry["latest_call"]["token_accounting_convention"],
            "inclusive"
        );
        assert_eq!(telemetry["cache_signal"]["state"], "cache_reset_suspected");
        assert_eq!(
            telemetry["warnings"]["items"][0]["code"],
            "malformed_record"
        );
        assert_eq!(container_logs_status, StatusCode::OK);
        assert_eq!(container_logs["lines"].as_array().map(Vec::len), Some(2));
        assert_eq!(deleted_status, StatusCode::NO_CONTENT);
        assert_eq!(after_status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn telemetry_route_returns_unavailable_for_unsupported_sessions() {
        let app = router_with_runtime(FakeRuntime::default());
        let (status, created) = json_request(
            &app,
            Method::POST,
            "/sessions",
            json!({ "harness": "echo" }),
        )
        .await;
        let session_id = session_id_from(&created);
        let (telemetry_status, telemetry) = empty_request(
            &app,
            Method::GET,
            &format!("/sessions/{session_id}/telemetry"),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(telemetry_status, StatusCode::OK);
        assert_eq!(telemetry["state"], "unavailable");
        assert!(
            telemetry["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("harness echo"))
        );
    }

    #[tokio::test]
    async fn telemetry_route_uses_standard_not_found_and_service_unavailable() {
        let runtime = FakeRuntime::default();
        let app = router_with_runtime(runtime.clone());
        let (_status, _created, session_id) = create_mounted_session(&app).await;

        let missing_status =
            status_request(&app, Method::GET, "/sessions/missing-session/telemetry").await;
        runtime.state().fail_telemetry = true;
        let (upstream_status, upstream_error) = empty_request(
            &app,
            Method::GET,
            &format!("/sessions/{session_id}/telemetry"),
        )
        .await;

        assert_eq!(missing_status, StatusCode::NOT_FOUND);
        assert_eq!(upstream_status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            upstream_error["detail"],
            "kernel telemetry provider is unavailable"
        );
    }

    #[tokio::test]
    async fn cleanup_route_reports_owned_session_authority() {
        let app = router_with_runtime(FakeRuntime::default());
        let (status, report) = json_request(
            &app,
            Method::POST,
            "/management/runtime-cleanup",
            json!({
                "owned_session_ids": ["one", "two"],
                "dry_run": false,
                "reviewed_resources": []
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(report["dry_run"], false);
        assert_eq!(report["owned_session_count"], 2);
        assert_eq!(report["resources"], json!([]));
    }

    #[tokio::test]
    async fn workspace_and_reset_routes_match_python_contract() {
        let app = router_with_runtime(FakeRuntime::default());
        let (_created_status, _created, session_id) = create_mounted_session(&app).await;

        let (snapshot_status, snapshot) = json_request(
            &app,
            Method::POST,
            &format!("/sessions/{session_id}/workspace/snapshot"),
            json!({
                "workspace_id": "saved-workspace",
                "volume_name": "agentspace-workspace-saved-workspace",
                "exclude_paths": [
                    "mounted-workspace",
                    ".github/agents/agentspace-session.agent.md"
                ]
            }),
        )
        .await;
        let (clone_status, cloned) = json_request(
            &app,
            Method::POST,
            "/workspaces/clone",
            json!({
                "source_volume_name": "agentspace-workspace-source",
                "target_workspace_id": "cloned-workspace",
                "target_volume_name": "agentspace-workspace-cloned-workspace"
            }),
        )
        .await;
        let (vscode_status, vscode) = json_request(
            &app,
            Method::POST,
            "/workspaces/vscode",
            json!({
                "workspace_id": "todo-list-code",
                "volume_name": "agentspace-workspace-todo-list-code"
            }),
        )
        .await;
        let (reset_status, reset) =
            empty_request(&app, Method::POST, &format!("/sessions/{session_id}/reset")).await;
        let reset_session_id = session_id_from(&reset);

        assert_eq!(snapshot_status, StatusCode::OK);
        assert_eq!(snapshot["workspace_id"], "saved-workspace");
        assert_eq!(
            snapshot["exclude_paths"],
            json!([
                "mounted-workspace",
                ".github/agents/agentspace-session.agent.md"
            ])
        );
        assert_eq!(clone_status, StatusCode::OK);
        assert_eq!(cloned["workspace_id"], "cloned-workspace");
        assert_eq!(vscode_status, StatusCode::OK);
        assert_eq!(vscode["container_name"], "editor-todo-list-code");
        assert_eq!(reset_status, StatusCode::OK);
        assert_eq!(reset_session_id, session_id);
    }

    #[tokio::test]
    async fn workspace_snapshot_rejects_unsafe_exclude_paths() {
        let app = router_with_runtime(FakeRuntime::default());
        let (_created_status, _created, session_id) = create_mounted_session(&app).await;

        for invalid in [
            "../secret",
            "/absolute",
            ".github/../secret",
            r".github\agents\profile",
            "",
        ] {
            let (status, payload) = json_request(
                &app,
                Method::POST,
                &format!("/sessions/{session_id}/workspace/snapshot"),
                json!({
                    "workspace_id": "saved-workspace",
                    "volume_name": "agentspace-workspace-saved-workspace",
                    "exclude_paths": [invalid]
                }),
            )
            .await;

            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(
                payload["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("exclude_paths"))
            );
        }
    }

    #[tokio::test]
    async fn session_stream_route_returns_ndjson_headers_and_events() {
        let app = router_with_runtime(FakeRuntime::default());
        let (_created_status, created) =
            json_request(&app, Method::POST, "/sessions", json!({})).await;
        let session_id = created["session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("created session should include session_id"));

        let response = raw_json_request(
            &app,
            Method::POST,
            &format!("/sessions/{session_id}/messages/stream"),
            json!({"message": "hello"}),
        )
        .await;
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let cache_control = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let x_accel_buffering = response
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("failed to read stream body: {error}"))
            .to_bytes();
        let lines = std::str::from_utf8(&body)
            .unwrap_or_else(|error| panic!("stream body should be UTF-8: {error}"))
            .lines()
            .map(|line| {
                serde_json::from_str::<JsonValue>(line)
                    .unwrap_or_else(|error| panic!("stream line should be JSON: {error}"))
            })
            .collect::<Vec<_>>();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "application/x-ndjson");
        assert_eq!(cache_control, "no-cache");
        assert_eq!(x_accel_buffering, "no");
        assert_eq!(lines[2]["content"], "hello");
        assert!(lines[2].get("error").is_none());
    }

    #[tokio::test]
    async fn session_routes_validate_workspace_mounts_and_log_tail() {
        let app = router_with_runtime(FakeRuntime::default());

        let invalid_workspace_status = status_json_request(
            &app,
            Method::POST,
            "/sessions",
            json!({"workspace_mounts": [{"workspace_id": "Bad Workspace"}]}),
        )
        .await;
        let invalid_mode_status = status_json_request(
            &app,
            Method::POST,
            "/sessions",
            json!({"workspace_mounts": [{"workspace_id": "ok-workspace", "mode": "bad"}]}),
        )
        .await;
        let invalid_volume_status = status_json_request(
            &app,
            Method::POST,
            "/sessions",
            json!({
                "workspace_mounts": [
                    {"workspace_id": "ok-workspace", "volume_name": "-bad"}
                ]
            }),
        )
        .await;
        let (_created_status, created) =
            json_request(&app, Method::POST, "/sessions", json!({})).await;
        let session_id = created["session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("created session should include session_id"));
        let invalid_tail_status = status_request(
            &app,
            Method::GET,
            &format!("/sessions/{session_id}/container-logs?tail=0"),
        )
        .await;

        assert_eq!(invalid_workspace_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid_mode_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid_volume_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid_tail_status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    fn session_id_from(payload: &JsonValue) -> String {
        payload["session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("payload should include session_id"))
            .to_owned()
    }

    async fn create_mounted_session(app: &Router) -> (StatusCode, JsonValue, String) {
        let (created_status, created) = json_request(
            app,
            Method::POST,
            "/sessions",
            json!({
                "harness": "copilot-cli",
                "workspace_mounts": [
                    {"workspace_id": "todo-list-code", "mode": "rw"}
                ]
            }),
        )
        .await;
        let session_id = session_id_from(&created);
        (created_status, created, session_id)
    }

    fn router_with_runtime(runtime: FakeRuntime) -> Router {
        let mut state = AppState::new(AppConfig::new("127.0.0.1", 0, BTreeMap::new()));
        state.sessions = SessionRegistry::with_runtime(Arc::new(runtime));
        build_router(state)
    }

    async fn json_request(
        app: &Router,
        method: Method,
        uri: &str,
        payload: JsonValue,
    ) -> (StatusCode, JsonValue) {
        let response = raw_json_request(app, method, uri, payload).await;
        response_parts(response).await
    }

    async fn status_json_request(
        app: &Router,
        method: Method,
        uri: &str,
        payload: JsonValue,
    ) -> StatusCode {
        let (status, _body) = json_request(app, method, uri, payload).await;
        status
    }

    async fn empty_request(app: &Router, method: Method, uri: &str) -> (StatusCode, JsonValue) {
        let response = raw_request(app, method, uri, Body::empty(), false).await;
        response_parts(response).await
    }

    async fn status_request(app: &Router, method: Method, uri: &str) -> StatusCode {
        let (status, _body) = empty_request(app, method, uri).await;
        status
    }

    async fn raw_json_request(
        app: &Router,
        method: Method,
        uri: &str,
        payload: JsonValue,
    ) -> axum::response::Response {
        let body = serde_json::to_vec(&payload)
            .unwrap_or_else(|error| panic!("failed to serialize request body: {error}"));
        raw_request(app, method, uri, Body::from(body), true).await
    }

    async fn raw_request(
        app: &Router,
        method: Method,
        uri: &str,
        body: Body,
        json_body: bool,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if json_body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        let request = builder
            .body(body)
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));
        app.clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"))
    }

    async fn response_parts(response: axum::response::Response) -> (StatusCode, JsonValue) {
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
            .to_bytes();
        let payload = if body.is_empty() {
            JsonValue::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("failed to parse response body: {error}"))
        };

        (status, payload)
    }

    fn session_key(session: &KernelRuntimeSession) -> String {
        match session {
            KernelRuntimeSession::Opaque(value) => value.clone(),
            KernelRuntimeSession::Docker(handle) => handle.container_name.clone(),
        }
    }

    fn session_start(session_id: &str, kernel: &str) -> KernelEvent {
        let mut event = KernelEvent::new(KernelEventType::SessionStart);
        event.session_id = Some(session_id.to_owned());
        event.kernel = Some(kernel.to_owned());
        event
    }

    fn status_event(status: KernelStatus) -> KernelEvent {
        let mut event = KernelEvent::new(KernelEventType::SessionStatus);
        event.status = Some(status);
        event
    }

    fn text_delta(content: &str) -> KernelEvent {
        let mut event = KernelEvent::new(KernelEventType::TextDelta);
        event.content = Some(content.to_owned());
        event
    }

    fn session_end() -> KernelEvent {
        KernelEvent::new(KernelEventType::SessionEnd)
    }
}
