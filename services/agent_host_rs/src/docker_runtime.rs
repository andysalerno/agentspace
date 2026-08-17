use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::Duration,
};

use async_stream::try_stream;
use async_trait::async_trait;
use bollard::{
    Docker,
    container::LogOutput,
    errors::Error as BollardError,
    exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults},
    models::{
        ContainerCreateBody, HostConfig, PortBinding as DockerPortBinding, PortMap,
        VolumeCreateRequest,
    },
    query_parameters::{
        CreateContainerOptionsBuilder, ListContainersOptionsBuilder, ListVolumesOptionsBuilder,
        LogsOptionsBuilder, RemoveContainerOptionsBuilder, RemoveVolumeOptionsBuilder,
        StatsOptionsBuilder, WaitContainerOptionsBuilder,
    },
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{sync::Mutex as AsyncMutex, time};
use uuid::Uuid;

use crate::{
    errors::AgentHostError,
    models::{
        CleanupAction, CleanupReport, CleanupResource, CleanupResourceIdentity,
        CleanupResourceKind, DockerKernelSession, DockerStatsSummary, HarnessName, InteractionMode,
        KernelEvent, KernelRuntimeSession, RuntimeSessionSummary, ServiceSummary,
        TelemetrySnapshot, WorkspaceMount,
    },
    sessions::{EventStream, KernelRuntime, RuntimeCreateSession},
    terminal::{ATTACHMENT_ID_ENV, TerminalExec, TerminalStatus},
};

const SESSION_WORKSPACE_MOUNT_PATH: &str = "/workspace";
const LABEL_INTERACTION_MODE: &str = "agentspace.interaction_mode";
const LABEL_MANAGED: &str = "agentspace.managed";
const LABEL_RESOURCE_ID: &str = "agentspace.resource_id";
const LABEL_ROLE: &str = "agentspace.role";
const LABEL_SESSION_ID: &str = "agentspace.session_id";
const RUNTIME_RECOVERY_ENV: &str = "AGENTSPACE_RUNTIME_RECOVERY";
const TERMINAL_RESUME_ENV: &str = "KERNEL_TERMINAL_RESUME";
const CONTAINER_NAME_PLACEHOLDER: &str = concat!("{", "container_name", "}");
const HOST_IP_PLACEHOLDER: &str = concat!("{", "host_ip", "}");
const HOST_PORT_PLACEHOLDER: &str = concat!("{", "host_port", "}");
const CONTAINER_PORT_PLACEHOLDER: &str = concat!("{", "container_port", "}");
const WORKSPACE_SNAPSHOT_SCRIPT: &str = include_str!("../scripts/snapshot_workspace.py");
const SESSION_TELEMETRY_MOUNT_PATH: &str = "/var/lib/agentspace/telemetry";

pub type DockerRuntime = DockerKernelRuntime;

#[derive(Clone)]
pub struct DockerKernelRuntime {
    config: DockerRuntimeConfig,
    backend: Arc<dyn DockerBackend>,
    client: reqwest::Client,
    workspace_editor_locks: Arc<Mutex<BTreeMap<String, Arc<AsyncMutex<()>>>>>,
}

impl Default for DockerKernelRuntime {
    fn default() -> Self {
        Self::from_env()
    }
}

impl DockerKernelRuntime {
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(
            DockerRuntimeConfig::from_env(),
            Arc::new(BollardDockerBackend::default()),
        )
    }

    #[must_use]
    pub fn new(config: DockerRuntimeConfig, backend: Arc<dyn DockerBackend>) -> Self {
        Self {
            config,
            backend,
            client: reqwest::Client::new(),
            workspace_editor_locks: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    #[must_use]
    pub const fn summary(&self) -> ServiceSummary {
        ServiceSummary::ready("Docker kernel runtime is active")
    }

    async fn run_kernel_container(
        &self,
        container_name: &str,
        session_workspace_volume: &str,
        session_telemetry_volume: Option<&str>,
        request: &RuntimeCreateSession,
    ) -> Result<(), AgentHostError> {
        let environment = self.kernel_environment(request);
        let ports = self.kernel_ports(&environment);
        let volumes = self
            .kernel_volumes(session_workspace_volume, session_telemetry_volume, request)
            .await?;

        self.backend
            .run_container(ContainerRunSpec {
                image: self.config.kernel_image.clone(),
                auto_remove: true,
                detach: true,
                entrypoint: kernel_entrypoint(),
                environment,
                labels: kernel_labels(request),
                name: Some(container_name.to_owned()),
                network: Some(self.config.kernel_network.clone()),
                network_disabled: false,
                ports,
                volumes,
            })
            .await
    }

    fn kernel_environment(&self, request: &RuntimeCreateSession) -> BTreeMap<String, String> {
        let mut environment = request.env.clone();
        let recovery_expected = environment.remove(RUNTIME_RECOVERY_ENV).is_some();
        environment.insert("KERNEL_HARNESS".to_owned(), request.harness.to_string());
        environment.insert(
            "AGENTSPACE_SESSION_ID".to_owned(),
            request.session_id.clone(),
        );
        if request.harness == HarnessName::CopilotCli {
            environment
                .entry("KERNEL_SESSION_ID".to_owned())
                .or_insert_with(|| request.session_id.clone());
            if recovery_expected {
                environment.insert(TERMINAL_RESUME_ENV.to_owned(), "1".to_owned());
            }
        }
        if !request.additional_paths.is_empty() {
            environment.insert(
                "KERNEL_ADDITIONAL_PATHS".to_owned(),
                request.additional_paths.join(path_separator()),
            );
        }
        environment.insert(
            "KERNEL_SKILLS_DIR".to_owned(),
            skills_mount_path(request.harness).to_owned(),
        );
        environment.insert(
            "KERNEL_SKILLS_STAGING_DIR".to_owned(),
            "/mnt/all-skills".to_owned(),
        );
        if request.harness == HarnessName::CopilotCli {
            environment.insert(
                "KERNEL_LEGACY_COPILOT_SKILLS_DIR".to_owned(),
                "/root/.copilot/skills".to_owned(),
            );
        }
        environment.insert("KERNEL_ENABLED_SKILLS".to_owned(), request.skills.join(","));
        environment
            .entry("KERNEL_VSCODE_ENABLED".to_owned())
            .or_insert_with(|| "1".to_owned());
        environment.insert(
            "KERNEL_FREE_PORT".to_owned(),
            self.config.free_port_container_port.to_string(),
        );
        environment
    }

    fn kernel_ports(&self, environment: &BTreeMap<String, String>) -> Vec<PortBinding> {
        let mut ports = vec![PortBinding {
            container_port: self.config.free_port_container_port,
            host_ip: self.config.free_port_host_ip.clone(),
        }];
        if vscode_enabled(environment.get("KERNEL_VSCODE_ENABLED")) {
            ports.push(PortBinding {
                container_port: self.config.vscode_container_port,
                host_ip: self.config.vscode_host_ip.clone(),
            });
        }
        ports
    }

    async fn kernel_volumes(
        &self,
        session_workspace_volume: &str,
        session_telemetry_volume: Option<&str>,
        request: &RuntimeCreateSession,
    ) -> Result<Vec<VolumeMount>, AgentHostError> {
        let mut volumes = vec![VolumeMount {
            volume_name: session_workspace_volume.to_owned(),
            bind: SESSION_WORKSPACE_MOUNT_PATH.to_owned(),
            mode: "rw".to_owned(),
        }];
        if let Some(session_telemetry_volume) = session_telemetry_volume {
            volumes.push(VolumeMount {
                volume_name: session_telemetry_volume.to_owned(),
                bind: SESSION_TELEMETRY_MOUNT_PATH.to_owned(),
                mode: "rw".to_owned(),
            });
        }
        volumes.extend([
            VolumeMount {
                volume_name: self.config.copilot_volume.clone(),
                bind: "/root/.copilot".to_owned(),
                mode: "rw".to_owned(),
            },
            VolumeMount {
                volume_name: self.config.skills_volume.clone(),
                bind: "/mnt/all-skills".to_owned(),
                mode: "ro".to_owned(),
            },
        ]);
        for mount in &request.workspace_mounts {
            volumes.push(self.workspace_volume_mount(mount).await?);
        }
        for resource in &request.skill_volumes {
            volumes.push(self.skill_volume_mount(resource).await?);
        }
        Ok(volumes)
    }

    async fn ensure_session_workspace_volume(
        &self,
        request: &RuntimeCreateSession,
    ) -> Result<String, AgentHostError> {
        let expected_name = session_workspace_volume_name(&request.session_id);
        if let Some(volume) = self.backend.inspect_volume(&expected_name).await?
            && volume.labels.get(LABEL_SESSION_ID) != Some(&request.session_id)
        {
            return Err(identity_collision(
                "volume name",
                &expected_name,
                &request.session_id,
            ));
        }

        let mut matching = self
            .backend
            .list_volumes()
            .await?
            .into_iter()
            .filter(|volume| {
                is_managed_role(&volume.labels, "session-workspace")
                    && volume.labels.get(LABEL_SESSION_ID) == Some(&request.session_id)
            })
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Err(AgentHostError::conflict(format!(
                "multiple Docker volumes claim session identity {:?}",
                request.session_id
            )));
        }
        if let Some(volume) = matching.pop() {
            validate_session_resource_labels(
                "Docker volume",
                &volume.name,
                &volume.labels,
                "session-workspace",
                request.interaction_mode,
            )?;
            return Ok(volume.name);
        }
        if recovery_expected(request) {
            return Err(AgentHostError::conflict(format!(
                "durable session workspace volume for session {:?} is missing",
                request.session_id
            )));
        }

        let mut labels = session_workspace_labels(request);
        labels.insert(LABEL_RESOURCE_ID.to_owned(), Uuid::now_v7().to_string());
        self.backend.ensure_volume(&expected_name, labels).await?;
        let created = self
            .backend
            .inspect_volume(&expected_name)
            .await?
            .ok_or_else(|| {
                AgentHostError::runtime(format!(
                    "Docker volume {expected_name:?} disappeared during creation"
                ))
            })?;
        if created.labels.get(LABEL_SESSION_ID) != Some(&request.session_id) {
            return Err(identity_collision(
                "volume name",
                &expected_name,
                &request.session_id,
            ));
        }
        validate_session_resource_labels(
            "Docker volume",
            &created.name,
            &created.labels,
            "session-workspace",
            request.interaction_mode,
        )?;
        Ok(expected_name)
    }

    async fn ensure_session_telemetry_volume(
        &self,
        request: &RuntimeCreateSession,
    ) -> Result<Option<String>, AgentHostError> {
        let Some(telemetry_volume_identity) = request.telemetry_volume_identity.as_deref() else {
            return Ok(None);
        };
        let expected_name = session_telemetry_volume_name(telemetry_volume_identity);
        if let Some(volume) = self.backend.inspect_volume(&expected_name).await?
            && volume.labels.get(LABEL_SESSION_ID) != Some(&request.session_id)
        {
            return Err(identity_collision(
                "volume name",
                &expected_name,
                &request.session_id,
            ));
        }

        let mut matching = self
            .backend
            .list_volumes()
            .await?
            .into_iter()
            .filter(|volume| {
                is_managed_role(&volume.labels, "session-telemetry")
                    && volume.labels.get(LABEL_SESSION_ID) == Some(&request.session_id)
            })
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Err(AgentHostError::conflict(format!(
                "multiple Docker telemetry volumes claim session identity {:?}",
                request.session_id
            )));
        }
        if let Some(volume) = matching.pop() {
            validate_telemetry_volume_labels("Docker volume", &volume.name, &volume.labels)?;
            return Ok(Some(volume.name));
        }
        if recovery_expected(request) {
            return Err(AgentHostError::conflict(format!(
                "durable session telemetry volume for session {:?} is missing",
                request.session_id
            )));
        }

        self.backend
            .ensure_volume(&expected_name, session_telemetry_labels(request))
            .await?;
        let created = self
            .backend
            .inspect_volume(&expected_name)
            .await?
            .ok_or_else(|| {
                AgentHostError::runtime(format!(
                    "Docker volume {expected_name:?} disappeared during creation"
                ))
            })?;
        if created.labels.get(LABEL_SESSION_ID) != Some(&request.session_id) {
            return Err(identity_collision(
                "volume name",
                &expected_name,
                &request.session_id,
            ));
        }
        validate_telemetry_volume_labels("Docker volume", &created.name, &created.labels)?;
        Ok(Some(expected_name))
    }

    async fn ensure_copilot_state_volume(
        &self,
        request: &RuntimeCreateSession,
    ) -> Result<(), AgentHostError> {
        if request.harness != HarnessName::CopilotCli {
            return Ok(());
        }
        if self
            .backend
            .inspect_volume(&self.config.copilot_volume)
            .await?
            .is_some()
        {
            return Ok(());
        }
        if recovery_expected(request) {
            return Err(AgentHostError::conflict(format!(
                "durable Copilot state volume {:?} is missing",
                self.config.copilot_volume
            )));
        }
        self.backend
            .ensure_volume(
                &self.config.copilot_volume,
                btree_map([(LABEL_ROLE, "copilot-state"), (LABEL_MANAGED, "true")]),
            )
            .await
    }

    async fn matching_kernel_container(
        &self,
        request: &RuntimeCreateSession,
    ) -> Result<Option<DockerContainerResource>, AgentHostError> {
        let expected_name = kernel_container_name(&request.session_id);
        if let Some(container) = self.backend.inspect_container(&expected_name).await?
            && container.labels.get(LABEL_SESSION_ID) != Some(&request.session_id)
        {
            return Err(identity_collision(
                "container name",
                &expected_name,
                &request.session_id,
            ));
        }

        let mut matching = self
            .backend
            .list_containers()
            .await?
            .into_iter()
            .filter(|container| container.labels.get(LABEL_SESSION_ID) == Some(&request.session_id))
            .collect::<Vec<_>>();
        if matching.len() > 1 {
            return Err(AgentHostError::conflict(format!(
                "multiple Docker containers claim session identity {:?}",
                request.session_id
            )));
        }
        let Some(container) = matching.pop() else {
            return Ok(None);
        };
        validate_session_resource_labels(
            "Docker container",
            &container.name,
            &container.labels,
            "kernel",
            request.interaction_mode,
        )?;
        if container
            .labels
            .get("agentspace.harness")
            .map(String::as_str)
            != Some(request.harness.as_str())
        {
            return Err(AgentHostError::conflict(format!(
                "Docker container {:?} has an incompatible AgentSpace harness label",
                container.name
            )));
        }
        Ok(Some(container))
    }

    async fn skill_volume_mount(
        &self,
        resource: &crate::skills::SkillVolumeResource,
    ) -> Result<VolumeMount, AgentHostError> {
        let key = format!("{}/{}", resource.skill_id, resource.resource_id);
        let volume_name = self
            .config
            .skill_volume_overrides
            .get(&key)
            .cloned()
            .unwrap_or_else(|| {
                format!("agentspace-{}-{}", resource.skill_id, resource.resource_id)
            });
        self.backend
            .ensure_volume(
                &volume_name,
                btree_map([
                    ("agentspace.role", "skill-resource"),
                    ("agentspace.managed", "true"),
                    ("agentspace.skill_id", resource.skill_id.as_str()),
                    ("agentspace.resource_id", resource.resource_id.as_str()),
                ]),
            )
            .await?;
        Ok(VolumeMount {
            volume_name,
            bind: resource.mount_path.clone(),
            mode: resource.mode.to_string(),
        })
    }

    async fn workspace_volume_mount(
        &self,
        mount: &WorkspaceMount,
    ) -> Result<VolumeMount, AgentHostError> {
        let volume_name = mount.effective_volume_name();
        self.backend
            .ensure_volume(
                &volume_name,
                btree_map([
                    ("agentspace.role", "workspace"),
                    ("agentspace.managed", "true"),
                ]),
            )
            .await?;
        Ok(VolumeMount {
            volume_name,
            bind: mount.mount_path(),
            mode: mount.mode.to_string(),
        })
    }

    async fn wait_until_ready(&self, base_url: &str) -> Result<(), AgentHostError> {
        let deadline = time::Instant::now() + self.config.startup_timeout;
        loop {
            match self.client.get(format!("{base_url}/healthz")).send().await {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(_) | Err(_) => {}
            }
            if time::Instant::now() >= deadline {
                return Err(AgentHostError::runtime(format!(
                    "kernel container at {base_url} did not become ready"
                )));
            }
            time::sleep(Duration::from_secs(1)).await;
        }
    }

    fn docker_session(
        session: &KernelRuntimeSession,
    ) -> Result<&DockerKernelSession, AgentHostError> {
        match session {
            KernelRuntimeSession::Docker(handle) => Ok(handle),
            KernelRuntimeSession::Opaque(_) => Err(AgentHostError::runtime(
                "unsupported runtime session handle for Docker runtime",
            )),
        }
    }

    async fn url_for_container_port(
        &self,
        container_name: &str,
        container_port: u16,
        host_ip: &str,
        template: &str,
    ) -> Result<Option<String>, AgentHostError> {
        let Some(host_port) = self
            .backend
            .container_host_port(container_name, container_port)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(format_runtime_template(
            template,
            container_name,
            host_ip,
            host_port,
            container_port,
        )))
    }

    async fn copy_workspace_volume(
        &self,
        source_volume_name: &str,
        target_workspace_id: &str,
        target_volume_name: &str,
        exclude_paths: &[String],
    ) -> Result<(), AgentHostError> {
        if source_volume_name == target_volume_name {
            return Err(AgentHostError::validation(
                "workspace snapshot source and destination volumes must be different",
            ));
        }
        self.backend.require_volume(source_volume_name).await?;
        self.backend
            .ensure_volume(
                target_volume_name,
                btree_map([
                    ("agentspace.role", "workspace"),
                    ("agentspace.managed", "true"),
                ]),
            )
            .await?;
        self.backend
            .run_container(ContainerRunSpec {
                image: self.config.kernel_image.clone(),
                auto_remove: true,
                detach: false,
                entrypoint: vec![
                    "/usr/local/bin/python".to_owned(),
                    "-c".to_owned(),
                    WORKSPACE_SNAPSHOT_SCRIPT.to_owned(),
                ],
                environment: btree_map([
                    ("AGENTSPACE_WORKSPACE_ID", target_workspace_id),
                    (
                        "AGENTSPACE_WORKSPACE_EXCLUDE_PATHS_JSON",
                        &serde_json::to_string(exclude_paths)?,
                    ),
                ]),
                labels: btree_map([("agentspace.role", "workspace-snapshot")]),
                name: None,
                network: None,
                network_disabled: true,
                ports: Vec::new(),
                volumes: vec![
                    VolumeMount {
                        volume_name: source_volume_name.to_owned(),
                        bind: "/workspace-src".to_owned(),
                        mode: "ro".to_owned(),
                    },
                    VolumeMount {
                        volume_name: target_volume_name.to_owned(),
                        bind: "/workspace-dest".to_owned(),
                        mode: "rw".to_owned(),
                    },
                ],
            })
            .await
    }

    async fn delete_reviewed_orphans(
        &self,
        owned_session_ids: &BTreeSet<String>,
        reviewed_resources: &[CleanupResourceIdentity],
    ) -> Result<CleanupReport, AgentHostError> {
        let unique = reviewed_resources.iter().cloned().collect::<BTreeSet<_>>();
        if unique.len() != reviewed_resources.len() {
            return Err(AgentHostError::validation(
                "reviewed_resources must not contain duplicates",
            ));
        }

        let mut resources = Vec::with_capacity(unique.len());
        for reviewed in unique {
            if reviewed
                .session_id
                .as_ref()
                .is_some_and(|session_id| owned_session_ids.contains(session_id))
            {
                resources.push(cleanup_resource(
                    reviewed.kind,
                    reviewed.name,
                    reviewed.resource_id,
                    &BTreeMap::new(),
                    None,
                    false,
                    Err(AgentHostError::conflict(
                        "reviewed runtime resource is now owned by a durable session",
                    )),
                ));
                continue;
            }
            let resource = match reviewed.kind {
                CleanupResourceKind::KernelContainer => {
                    let current = self.backend.inspect_container(&reviewed.name).await?;
                    let labels = current
                        .as_ref()
                        .map(|resource| resource.labels.clone())
                        .unwrap_or_default();
                    let status = current.as_ref().map(|resource| resource.status.clone());
                    let result = match validate_reviewed_container(current.as_ref(), &reviewed) {
                        Ok(()) => self.backend.remove_container(&reviewed.name).await,
                        Err(error) => Err(error),
                    };
                    cleanup_resource(
                        reviewed.kind,
                        reviewed.name,
                        reviewed.resource_id,
                        &labels,
                        status,
                        false,
                        result,
                    )
                }
                CleanupResourceKind::SessionWorkspaceVolume
                | CleanupResourceKind::SessionTelemetryVolume => {
                    let current = self.backend.inspect_volume(&reviewed.name).await?;
                    let labels = current
                        .as_ref()
                        .map(|resource| resource.labels.clone())
                        .unwrap_or_default();
                    let result = match validate_reviewed_volume(
                        current.as_ref(),
                        &reviewed,
                        reviewed.kind,
                    ) {
                        Ok(()) => self.backend.remove_volume(&reviewed.name).await,
                        Err(error) => Err(error),
                    };
                    cleanup_resource(
                        reviewed.kind,
                        reviewed.name,
                        reviewed.resource_id,
                        &labels,
                        None,
                        false,
                        result,
                    )
                }
            };
            resources.push(resource);
        }
        let deleted_count = resources
            .iter()
            .filter(|resource| resource.action == CleanupAction::Deleted)
            .count();
        let error_count = resources
            .iter()
            .filter(|resource| resource.action == CleanupAction::DeleteFailed)
            .count();
        Ok(CleanupReport {
            dry_run: false,
            owned_session_count: owned_session_ids.len(),
            resources,
            deleted_count,
            error_count,
        })
    }

    async fn open_workspace_vscode_locked(
        &self,
        workspace_id: &str,
        volume_name: &str,
    ) -> Result<(String, Option<String>), AgentHostError> {
        self.backend.require_volume(volume_name).await?;
        let container_name = workspace_editor_container_name(workspace_id);
        if !self.backend.container_is_running(&container_name).await? {
            self.backend.remove_container(&container_name).await?;
            self.backend
                .run_container(ContainerRunSpec {
                    image: self.config.kernel_image.clone(),
                    auto_remove: true,
                    detach: true,
                    entrypoint: vec![
                        "/usr/local/bin/code-server".to_owned(),
                        "--bind-addr".to_owned(),
                        format!("0.0.0.0:{}", self.config.vscode_container_port),
                        "--auth".to_owned(),
                        "none".to_owned(),
                        "--disable-telemetry".to_owned(),
                        "/workspace".to_owned(),
                    ],
                    environment: BTreeMap::new(),
                    labels: btree_map([
                        ("agentspace.role", "workspace-editor"),
                        ("agentspace.workspace_id", workspace_id),
                    ]),
                    name: Some(container_name.clone()),
                    network: Some(self.config.kernel_network.clone()),
                    network_disabled: false,
                    ports: vec![PortBinding {
                        container_port: self.config.vscode_container_port,
                        host_ip: self.config.vscode_host_ip.clone(),
                    }],
                    volumes: vec![VolumeMount {
                        volume_name: volume_name.to_owned(),
                        bind: "/workspace".to_owned(),
                        mode: "rw".to_owned(),
                    }],
                })
                .await?;
        }
        let vscode_url = self
            .url_for_container_port(
                &container_name,
                self.config.vscode_container_port,
                &self.config.vscode_host_ip,
                &self.config.vscode_url_template,
            )
            .await?;
        Ok((container_name, vscode_url))
    }

    async fn terminal_request(
        &self,
        session: &KernelRuntimeSession,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        missing_attachment_id: Option<&str>,
    ) -> Result<TerminalStatus, AgentHostError> {
        let handle = Self::docker_session(session)?;
        let mut request = self
            .client
            .request(method, format!("{}{path}", handle.base_url));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| {
            AgentHostError::upstream_unavailable(format!(
                "kernel terminal controller is unavailable: {error}"
            ))
        })?;
        if response.status().is_success() {
            return response.json::<TerminalStatus>().await.map_err(|error| {
                AgentHostError::upstream_unavailable(format!(
                    "kernel terminal controller returned an invalid response: {error}"
                ))
            });
        }

        let status = response.status();
        let detail = response
            .json::<KernelErrorResponse>()
            .await
            .ok()
            .map_or_else(
                || format!("kernel terminal controller returned HTTP {status}"),
                |payload| payload.detail,
            );
        match status.as_u16() {
            404 => missing_attachment_id.map_or_else(
                || Err(AgentHostError::upstream_unavailable(detail)),
                |attachment_id| Err(AgentHostError::terminal_attachment_not_found(attachment_id)),
            ),
            409 => Err(AgentHostError::conflict(detail)),
            _ => Err(AgentHostError::upstream_unavailable(detail)),
        }
    }

    async fn telemetry_request(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<TelemetrySnapshot, AgentHostError> {
        let handle = Self::docker_session(session)?;
        let response = self
            .client
            .get(format!("{}/telemetry", handle.base_url))
            .send()
            .await
            .map_err(|error| {
                AgentHostError::upstream_unavailable(format!(
                    "kernel telemetry provider is unavailable: {error}"
                ))
            })?;
        if response.status().is_success() {
            return response.json::<TelemetrySnapshot>().await.map_err(|error| {
                AgentHostError::upstream_unavailable(format!(
                    "kernel telemetry provider returned an invalid response: {error}"
                ))
            });
        }

        let status = response.status();
        let detail = response
            .json::<KernelErrorResponse>()
            .await
            .ok()
            .map_or_else(
                || format!("kernel telemetry provider returned HTTP {status}"),
                |payload| payload.detail,
            );
        Err(AgentHostError::upstream_unavailable(detail))
    }
}

#[async_trait]
impl KernelRuntime for DockerKernelRuntime {
    async fn create_session(
        &self,
        request: RuntimeCreateSession,
    ) -> Result<KernelRuntimeSession, AgentHostError> {
        self.ensure_copilot_state_volume(&request).await?;
        let workspace_volume_name = self.ensure_session_workspace_volume(&request).await?;
        let telemetry_volume_name = self.ensure_session_telemetry_volume(&request).await?;
        let matching_container = self.matching_kernel_container(&request).await?;
        let container_name = if let Some(container) = matching_container {
            if container.running {
                let mounted_workspace = container
                    .mounts
                    .get(SESSION_WORKSPACE_MOUNT_PATH)
                    .ok_or_else(|| {
                        AgentHostError::conflict(format!(
                            "Docker container {:?} for session {:?} has no session workspace mount",
                            container.name, request.session_id
                        ))
                    })?;
                if mounted_workspace != &workspace_volume_name {
                    return Err(AgentHostError::conflict(format!(
                        "Docker container {:?} for session {:?} mounts workspace volume {:?}, expected {:?}",
                        container.name,
                        request.session_id,
                        mounted_workspace,
                        workspace_volume_name
                    )));
                }
                if let Some(expected_telemetry_volume) = telemetry_volume_name.as_deref() {
                    let mounted_telemetry = container
                        .mounts
                        .get(SESSION_TELEMETRY_MOUNT_PATH)
                        .ok_or_else(|| {
                            AgentHostError::conflict(format!(
                                "Docker container {:?} for session {:?} has no session telemetry mount",
                                container.name, request.session_id
                            ))
                        })?;
                    if mounted_telemetry != expected_telemetry_volume {
                        return Err(AgentHostError::conflict(format!(
                            "Docker container {:?} for session {:?} mounts telemetry volume {:?}, expected {:?}",
                            container.name,
                            request.session_id,
                            mounted_telemetry,
                            expected_telemetry_volume
                        )));
                    }
                }
                container.name
            } else {
                self.backend.remove_container(&container.name).await?;
                let expected_name = kernel_container_name(&request.session_id);
                self.run_kernel_container(
                    &expected_name,
                    &workspace_volume_name,
                    telemetry_volume_name.as_deref(),
                    &request,
                )
                .await?;
                expected_name
            }
        } else {
            let expected_name = kernel_container_name(&request.session_id);
            self.run_kernel_container(
                &expected_name,
                &workspace_volume_name,
                telemetry_volume_name.as_deref(),
                &request,
            )
            .await?;
            expected_name
        };
        let base_url = self
            .config
            .base_url_template
            .replace(CONTAINER_NAME_PLACEHOLDER, &container_name);
        let vscode_url = self
            .url_for_container_port(
                &container_name,
                self.config.vscode_container_port,
                &self.config.vscode_host_ip,
                &self.config.vscode_url_template,
            )
            .await?;
        let free_port_url = self
            .url_for_container_port(
                &container_name,
                self.config.free_port_container_port,
                &self.config.free_port_host_ip,
                &self.config.free_port_url_template,
            )
            .await?;
        self.wait_until_ready(&base_url).await?;
        Ok(KernelRuntimeSession::Docker(DockerKernelSession {
            session_id: request.session_id,
            container_name,
            session_workspace_volume_name: workspace_volume_name,
            session_telemetry_volume_name: telemetry_volume_name,
            base_url,
            vscode_url,
            free_port_url,
        }))
    }

    fn stream_message(
        &self,
        session: KernelRuntimeSession,
        message: String,
    ) -> Result<EventStream, AgentHostError> {
        let handle = Self::docker_session(&session)?.clone();
        let client = self.client.clone();
        Ok(Box::pin(try_stream! {
            let response = client
                .post(format!("{}/messages/stream", handle.base_url))
                .json(&json!({ "message": message }))
                .send()
                .await?
                .error_for_status()?;
            let mut chunks = response.bytes_stream();
            let mut buffer = Vec::new();
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk?;
                buffer.extend_from_slice(&chunk);
                while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line = buffer.drain(..=newline).collect::<Vec<_>>();
                    if let Some(event) = parse_event_line(&line[..line.len().saturating_sub(1)])? {
                        yield event;
                    }
                }
            }
            if let Some(event) = parse_event_line(&buffer)? {
                yield event;
            }
        }))
    }

    async fn summary(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<RuntimeSessionSummary, AgentHostError> {
        let handle = Self::docker_session(session)?;
        let summary = self
            .client
            .get(format!("{}/session", handle.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<RuntimeSessionSummary>()
            .await?;
        Ok(normalize_runtime_summary(summary))
    }

    async fn history(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<Vec<Vec<KernelEvent>>, AgentHostError> {
        let handle = Self::docker_session(session)?;
        let payload = self
            .client
            .get(format!("{}/history", handle.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<KernelHistoryResponse>()
            .await?;
        Ok(payload.history)
    }

    async fn logs(&self, session: &KernelRuntimeSession) -> Result<Vec<String>, AgentHostError> {
        let handle = Self::docker_session(session)?;
        let payload = self
            .client
            .get(format!("{}/logs", handle.base_url))
            .send()
            .await?
            .error_for_status()?
            .json::<KernelLogsResponse>()
            .await?;
        Ok(payload.lines)
    }

    async fn container_logs(
        &self,
        session: &KernelRuntimeSession,
        tail: Option<u32>,
    ) -> Result<Vec<String>, AgentHostError> {
        let handle = Self::docker_session(session)?;
        self.backend
            .container_logs(&handle.container_name, tail)
            .await
    }

    async fn stats(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<Option<DockerStatsSummary>, AgentHostError> {
        let handle = Self::docker_session(session)?;
        let Some(raw) = self.backend.container_stats(&handle.container_name).await? else {
            return Ok(None);
        };
        Ok(summarize_docker_stats(&raw))
    }

    fn container_name(&self, session: &KernelRuntimeSession) -> Option<String> {
        Self::docker_session(session)
            .ok()
            .map(|handle| handle.container_name.clone())
    }

    fn vscode_url(&self, session: &KernelRuntimeSession) -> Option<String> {
        Self::docker_session(session)
            .ok()
            .and_then(|handle| handle.vscode_url.clone())
    }

    fn vscode_proxy_base_url(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<String, AgentHostError> {
        let handle = Self::docker_session(session)?;
        Ok(self
            .config
            .vscode_proxy_url_template
            .replace(CONTAINER_NAME_PLACEHOLDER, &handle.container_name)
            .replace(
                CONTAINER_PORT_PLACEHOLDER,
                &self.config.vscode_container_port.to_string(),
            ))
    }

    fn free_port_url(&self, session: &KernelRuntimeSession) -> Option<String> {
        Self::docker_session(session)
            .ok()
            .and_then(|handle| handle.free_port_url.clone())
    }

    async fn destroy_session(&self, session: KernelRuntimeSession) -> Result<(), AgentHostError> {
        let handle = Self::docker_session(&session)?;
        if let Some(container) = self
            .backend
            .inspect_container(&handle.container_name)
            .await?
            && (!is_managed_role(&container.labels, "kernel")
                || container.labels.get(LABEL_SESSION_ID) != Some(&handle.session_id))
        {
            return Err(identity_collision(
                "container name",
                &handle.container_name,
                &handle.session_id,
            ));
        }
        if let Some(volume) = self
            .backend
            .inspect_volume(&handle.session_workspace_volume_name)
            .await?
            && (!is_managed_role(&volume.labels, "session-workspace")
                || volume.labels.get(LABEL_SESSION_ID) != Some(&handle.session_id))
        {
            return Err(identity_collision(
                "volume name",
                &handle.session_workspace_volume_name,
                &handle.session_id,
            ));
        }
        if let Some(session_telemetry_volume_name) = handle.session_telemetry_volume_name.as_deref()
            && let Some(volume) = self
                .backend
                .inspect_volume(session_telemetry_volume_name)
                .await?
            && (!is_managed_role(&volume.labels, "session-telemetry")
                || volume.labels.get(LABEL_SESSION_ID) != Some(&handle.session_id))
        {
            return Err(identity_collision(
                "volume name",
                session_telemetry_volume_name,
                &handle.session_id,
            ));
        }
        self.backend
            .remove_container(&handle.container_name)
            .await?;
        self.backend
            .remove_volume(&handle.session_workspace_volume_name)
            .await?;
        if let Some(session_telemetry_volume_name) = handle.session_telemetry_volume_name.as_deref()
        {
            self.backend
                .remove_volume(session_telemetry_volume_name)
                .await?;
        }
        Ok(())
    }

    async fn destroy_session_by_id(&self, session_id: &str) -> Result<(), AgentHostError> {
        let mut containers = self
            .backend
            .list_containers()
            .await?
            .into_iter()
            .filter(|container| {
                is_managed_role(&container.labels, "kernel")
                    && container.labels.get(LABEL_SESSION_ID).map(String::as_str)
                        == Some(session_id)
            })
            .collect::<Vec<_>>();
        let mut workspace_volumes = self
            .backend
            .list_volumes()
            .await?
            .into_iter()
            .filter(|volume| {
                is_managed_role(&volume.labels, "session-workspace")
                    && volume.labels.get(LABEL_SESSION_ID).map(String::as_str) == Some(session_id)
            })
            .collect::<Vec<_>>();
        let mut telemetry_volumes = self
            .backend
            .list_volumes()
            .await?
            .into_iter()
            .filter(|volume| {
                is_managed_role(&volume.labels, "session-telemetry")
                    && volume.labels.get(LABEL_SESSION_ID).map(String::as_str) == Some(session_id)
            })
            .collect::<Vec<_>>();
        if containers.len() > 1 || workspace_volumes.len() > 1 || telemetry_volumes.len() > 1 {
            return Err(AgentHostError::conflict(format!(
                "multiple managed Docker resources claim session identity {session_id:?}"
            )));
        }
        if containers.is_empty() && workspace_volumes.is_empty() && telemetry_volumes.is_empty() {
            return Err(AgentHostError::session_not_found(session_id));
        }
        if let Some(container) = containers.pop() {
            self.backend.remove_container(&container.name).await?;
        }
        if let Some(volume) = workspace_volumes.pop() {
            self.backend.remove_volume(&volume.name).await?;
        }
        if let Some(volume) = telemetry_volumes.pop() {
            self.backend.remove_volume(&volume.name).await?;
        }
        Ok(())
    }

    async fn cleanup_orphans(
        &self,
        owned_session_ids: &BTreeSet<String>,
        dry_run: bool,
        reviewed_resources: Option<&[CleanupResourceIdentity]>,
    ) -> Result<CleanupReport, AgentHostError> {
        if !dry_run {
            let reviewed_resources = reviewed_resources.ok_or_else(|| {
                AgentHostError::validation(
                    "destructive runtime cleanup requires reviewed_resources from a dry-run report",
                )
            })?;
            return self
                .delete_reviewed_orphans(owned_session_ids, reviewed_resources)
                .await;
        }
        let mut containers = self
            .backend
            .list_containers()
            .await?
            .into_iter()
            .filter(|container| {
                is_managed_role(&container.labels, "kernel")
                    && !resource_is_owned(&container.labels, owned_session_ids)
            })
            .collect::<Vec<_>>();
        containers.sort_by(|left, right| left.name.cmp(&right.name));
        let mut volumes = self
            .backend
            .list_volumes()
            .await?
            .into_iter()
            .filter(|volume| {
                is_managed_role(&volume.labels, "session-workspace")
                    && !resource_is_owned(&volume.labels, owned_session_ids)
            })
            .collect::<Vec<_>>();
        volumes.sort_by(|left, right| left.name.cmp(&right.name));
        let mut telemetry_volumes = self
            .backend
            .list_volumes()
            .await?
            .into_iter()
            .filter(|volume| {
                is_managed_role(&volume.labels, "session-telemetry")
                    && !resource_is_owned(&volume.labels, owned_session_ids)
            })
            .collect::<Vec<_>>();
        telemetry_volumes.sort_by(|left, right| left.name.cmp(&right.name));

        let mut resources =
            Vec::with_capacity(containers.len() + volumes.len() + telemetry_volumes.len());
        for container in containers {
            resources.push(cleanup_resource(
                CleanupResourceKind::KernelContainer,
                container.name,
                container.resource_id,
                &container.labels,
                Some(container.status),
                true,
                Ok(()),
            ));
        }
        for volume in volumes {
            resources.push(cleanup_resource(
                CleanupResourceKind::SessionWorkspaceVolume,
                volume.name,
                volume.resource_id,
                &volume.labels,
                None,
                true,
                Ok(()),
            ));
        }
        for volume in telemetry_volumes {
            resources.push(cleanup_resource(
                CleanupResourceKind::SessionTelemetryVolume,
                volume.name,
                volume.resource_id,
                &volume.labels,
                None,
                true,
                Ok(()),
            ));
        }
        let deleted_count = resources
            .iter()
            .filter(|resource| resource.action == CleanupAction::Deleted)
            .count();
        let error_count = resources
            .iter()
            .filter(|resource| resource.action == CleanupAction::DeleteFailed)
            .count();
        Ok(CleanupReport {
            dry_run,
            owned_session_count: owned_session_ids.len(),
            resources,
            deleted_count,
            error_count,
        })
    }

    async fn snapshot_session_workspace(
        &self,
        session: &KernelRuntimeSession,
        workspace_id: String,
        volume_name: String,
        exclude_paths: Vec<String>,
    ) -> Result<Value, AgentHostError> {
        let handle = Self::docker_session(session)?;
        self.copy_workspace_volume(
            &handle.session_workspace_volume_name,
            &workspace_id,
            &volume_name,
            &exclude_paths,
        )
        .await?;
        Ok(json!({ "workspace_id": workspace_id, "volume_name": volume_name }))
    }

    async fn clone_workspace(
        &self,
        source_volume_name: String,
        target_workspace_id: String,
        target_volume_name: String,
    ) -> Result<Value, AgentHostError> {
        self.copy_workspace_volume(
            &source_volume_name,
            &target_workspace_id,
            &target_volume_name,
            &[],
        )
        .await?;
        Ok(json!({
            "workspace_id": target_workspace_id,
            "volume_name": target_volume_name
        }))
    }

    async fn open_workspace_vscode(
        &self,
        workspace_id: String,
        volume_name: String,
    ) -> Result<Value, AgentHostError> {
        let lock = {
            let mut locks = self
                .workspace_editor_locks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            locks
                .entry(workspace_id.clone())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        let (container_name, vscode_url) = self
            .open_workspace_vscode_locked(&workspace_id, &volume_name)
            .await?;
        Ok(json!({
            "workspace_id": workspace_id,
            "volume_name": volume_name,
            "container_name": container_name,
            "vscode_url": vscode_url,
        }))
    }

    async fn terminal_status(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        self.terminal_request(session, reqwest::Method::GET, "/terminal", None, None)
            .await
    }

    async fn terminal_ensure(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        self.terminal_request(
            session,
            reqwest::Method::POST,
            "/terminal/ensure",
            None,
            None,
        )
        .await
    }

    async fn terminal_stop(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        self.terminal_request(session, reqwest::Method::POST, "/terminal/stop", None, None)
            .await
    }

    async fn terminal_resume(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<TerminalStatus, AgentHostError> {
        self.terminal_request(
            session,
            reqwest::Method::POST,
            "/terminal/resume",
            None,
            None,
        )
        .await
    }

    async fn terminal_detach_client(
        &self,
        session: &KernelRuntimeSession,
        tmux_client_id: &str,
    ) -> Result<TerminalStatus, AgentHostError> {
        self.terminal_request(
            session,
            reqwest::Method::POST,
            "/terminal/detach-client",
            Some(json!({ "tmux_client_id": tmux_client_id })),
            Some(tmux_client_id),
        )
        .await
    }

    async fn telemetry(
        &self,
        session: &KernelRuntimeSession,
    ) -> Result<TelemetrySnapshot, AgentHostError> {
        self.telemetry_request(session).await
    }

    async fn terminal_attach(
        &self,
        session: &KernelRuntimeSession,
        attachment_id: &str,
        attach_argv: &[String],
    ) -> Result<TerminalExec, AgentHostError> {
        let handle = Self::docker_session(session)?;
        self.backend
            .open_terminal_exec(&handle.container_name, attachment_id, attach_argv)
            .await
    }

    async fn terminal_resize(
        &self,
        session: &KernelRuntimeSession,
        exec_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), AgentHostError> {
        Self::docker_session(session)?;
        self.backend.resize_terminal_exec(exec_id, cols, rows).await
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerRuntimeConfig {
    pub kernel_image: String,
    pub kernel_network: String,
    pub base_url_template: String,
    pub vscode_container_port: u16,
    pub vscode_host_ip: String,
    pub vscode_url_template: String,
    pub vscode_proxy_url_template: String,
    pub free_port_container_port: u16,
    pub free_port_host_ip: String,
    pub free_port_url_template: String,
    pub startup_timeout: Duration,
    pub copilot_volume: String,
    pub skills_volume: String,
    pub skills_dir: String,
    pub skill_volume_overrides: BTreeMap<String, String>,
}

impl Default for DockerRuntimeConfig {
    fn default() -> Self {
        Self {
            kernel_image: "agentspace-kernel-kernel:latest".to_owned(),
            kernel_network: "agentspace-agent-host_default".to_owned(),
            base_url_template: "http://{container_name}:8000".to_owned(),
            vscode_container_port: 8080,
            vscode_host_ip: "0.0.0.0".to_owned(),
            vscode_url_template: "http://127.0.0.1:{host_port}".to_owned(),
            vscode_proxy_url_template: "http://{container_name}:{container_port}".to_owned(),
            free_port_container_port: 8081,
            free_port_host_ip: "0.0.0.0".to_owned(),
            free_port_url_template: "http://127.0.0.1:{host_port}".to_owned(),
            startup_timeout: Duration::from_mins(1),
            copilot_volume: "agentspace-kernel_copilot-config".to_owned(),
            skills_volume: "agentspace-skills".to_owned(),
            skills_dir: "/skills".to_owned(),
            skill_volume_overrides: BTreeMap::new(),
        }
    }
}

impl DockerRuntimeConfig {
    #[must_use]
    pub fn from_env() -> Self {
        let mut config = Self::default();
        config.kernel_image = env_or("AGENT_HOST_KERNEL_IMAGE", &config.kernel_image);
        config.kernel_network = env_or("AGENT_HOST_DOCKER_NETWORK", &config.kernel_network);
        config.base_url_template = env_or(
            "AGENT_HOST_KERNEL_BASE_URL_TEMPLATE",
            &config.base_url_template,
        );
        config.vscode_container_port = env_u16(
            "AGENT_HOST_KERNEL_VSCODE_CONTAINER_PORT",
            config.vscode_container_port,
        );
        config.vscode_host_ip = env_or("AGENT_HOST_KERNEL_VSCODE_HOST_IP", &config.vscode_host_ip);
        config.vscode_url_template = env_or(
            "AGENT_HOST_KERNEL_VSCODE_URL_TEMPLATE",
            &config.vscode_url_template,
        );
        config.vscode_proxy_url_template = env_or(
            "AGENT_HOST_KERNEL_VSCODE_PROXY_URL_TEMPLATE",
            &config.vscode_proxy_url_template,
        );
        config.free_port_container_port = env_u16(
            "AGENT_HOST_KERNEL_FREE_PORT_CONTAINER_PORT",
            config.free_port_container_port,
        );
        config.free_port_host_ip = std::env::var("AGENT_HOST_KERNEL_FREE_PORT_HOST_IP")
            .unwrap_or_else(|_| config.vscode_host_ip.clone());
        config.free_port_url_template = env_or(
            "AGENT_HOST_KERNEL_FREE_PORT_URL_TEMPLATE",
            &config.free_port_url_template,
        );
        config.startup_timeout = Duration::from_secs_f64(env_f64(
            "AGENT_HOST_KERNEL_STARTUP_TIMEOUT",
            config.startup_timeout.as_secs_f64(),
        ));
        config.copilot_volume = env_or("AGENT_HOST_COPILOT_VOLUME", &config.copilot_volume);
        config.skills_volume = env_or("AGENT_HOST_SKILLS_VOLUME", &config.skills_volume);
        config.skills_dir = env_or("AGENT_HOST_SKILLS_DIR", &config.skills_dir);
        config.skill_volume_overrides = Self::skill_volume_overrides_from_process();
        config
    }

    fn skill_volume_overrides_from_process() -> BTreeMap<String, String> {
        const PREFIX: &str = "AGENT_HOST_SKILL_VOLUME_";
        std::env::vars()
            .filter_map(|(name, value)| {
                let suffix = name.strip_prefix(PREFIX)?;
                let (skill_id, resource_id) = suffix.split_once("__")?;
                if skill_id.is_empty() || resource_id.is_empty() || value.is_empty() {
                    return None;
                }
                Some((
                    format!(
                        "{}/{}",
                        skill_id.to_ascii_lowercase().replace('_', "-"),
                        resource_id.to_ascii_lowercase().replace('_', "-")
                    ),
                    value,
                ))
            })
            .collect()
    }
}

#[async_trait]
pub trait DockerBackend: Send + Sync {
    async fn ensure_volume(
        &self,
        volume_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<(), AgentHostError>;

    async fn require_volume(&self, volume_name: &str) -> Result<(), AgentHostError>;

    async fn run_container(&self, spec: ContainerRunSpec) -> Result<(), AgentHostError>;

    async fn inspect_container(
        &self,
        container_name: &str,
    ) -> Result<Option<DockerContainerResource>, AgentHostError>;

    async fn list_containers(&self) -> Result<Vec<DockerContainerResource>, AgentHostError>;

    async fn inspect_volume(
        &self,
        volume_name: &str,
    ) -> Result<Option<DockerVolumeResource>, AgentHostError>;

    async fn list_volumes(&self) -> Result<Vec<DockerVolumeResource>, AgentHostError>;

    async fn container_host_port(
        &self,
        container_name: &str,
        container_port: u16,
    ) -> Result<Option<u16>, AgentHostError>;

    async fn container_is_running(&self, container_name: &str) -> Result<bool, AgentHostError>;

    async fn remove_container(&self, container_name: &str) -> Result<(), AgentHostError>;

    async fn remove_volume(&self, volume_name: &str) -> Result<(), AgentHostError>;

    async fn container_logs(
        &self,
        container_name: &str,
        tail: Option<u32>,
    ) -> Result<Vec<String>, AgentHostError>;

    async fn container_stats(
        &self,
        container_name: &str,
    ) -> Result<Option<DockerStats>, AgentHostError>;

    async fn open_terminal_exec(
        &self,
        _container_name: &str,
        _attachment_id: &str,
        _attach_argv: &[String],
    ) -> Result<TerminalExec, AgentHostError> {
        Err(AgentHostError::runtime(
            "terminal exec is not supported by this Docker backend",
        ))
    }

    async fn resize_terminal_exec(
        &self,
        _exec_id: &str,
        _cols: u16,
        _rows: u16,
    ) -> Result<(), AgentHostError> {
        Err(AgentHostError::runtime(
            "terminal exec resize is not supported by this Docker backend",
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerContainerResource {
    pub name: String,
    pub resource_id: String,
    pub labels: BTreeMap<String, String>,
    pub running: bool,
    pub status: String,
    pub mounts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerVolumeResource {
    pub name: String,
    pub resource_id: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerRunSpec {
    pub image: String,
    pub auto_remove: bool,
    pub detach: bool,
    pub entrypoint: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub name: Option<String>,
    pub network: Option<String>,
    pub network_disabled: bool,
    pub ports: Vec<PortBinding>,
    pub volumes: Vec<VolumeMount>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortBinding {
    pub container_port: u16,
    pub host_ip: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeMount {
    pub volume_name: String,
    pub bind: String,
    pub mode: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
struct KernelHistoryResponse {
    #[serde(default)]
    history: Vec<Vec<KernelEvent>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
struct KernelLogsResponse {
    #[serde(default)]
    lines: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct KernelErrorResponse {
    detail: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct DockerStats {
    #[serde(default, rename = "cpu_stats")]
    cpu: DockerCpuStats,
    #[serde(default, rename = "precpu_stats")]
    precpu: DockerCpuStats,
    #[serde(default, rename = "memory_stats")]
    memory: DockerMemoryStats,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct DockerCpuStats {
    #[serde(default)]
    cpu_usage: DockerCpuUsage,
    system_cpu_usage: Option<u64>,
    online_cpus: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct DockerCpuUsage {
    total_usage: Option<u64>,
    #[serde(default)]
    percpu_usage: Vec<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct DockerMemoryStats {
    usage: Option<u64>,
    limit: Option<u64>,
    #[serde(default)]
    stats: DockerMemoryStatDetails,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
struct DockerMemoryStatDetails {
    cache: Option<u64>,
    inactive_file: Option<u64>,
}

#[derive(Clone, Default)]
struct BollardDockerBackend {
    docker: Arc<OnceLock<Result<Docker, String>>>,
}

impl BollardDockerBackend {
    fn docker(&self) -> Result<Docker, AgentHostError> {
        let result = self
            .docker
            .get_or_init(|| Docker::connect_with_defaults().map_err(|error| error.to_string()));

        match result {
            Ok(docker) => Ok(docker.clone()),
            Err(message) => Err(AgentHostError::runtime(format!(
                "failed to connect to Docker: {message}"
            ))),
        }
    }
}

#[async_trait]
impl DockerBackend for BollardDockerBackend {
    async fn ensure_volume(
        &self,
        volume_name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<(), AgentHostError> {
        let docker = self.docker()?;
        match docker.inspect_volume(volume_name).await {
            Ok(_) => Ok(()),
            Err(error) if is_bollard_not_found(&error) => {
                docker
                    .create_volume(VolumeCreateRequest {
                        name: Some(volume_name.to_owned()),
                        labels: Some(hash_map_from_btree(&labels)),
                        ..VolumeCreateRequest::default()
                    })
                    .await?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn require_volume(&self, volume_name: &str) -> Result<(), AgentHostError> {
        let docker = self.docker()?;
        match docker.inspect_volume(volume_name).await {
            Ok(_) => Ok(()),
            Err(error) if is_bollard_not_found(&error) => Err(AgentHostError::runtime(format!(
                "Docker volume {volume_name:?} does not exist"
            ))),
            Err(error) => Err(error.into()),
        }
    }

    async fn run_container(&self, spec: ContainerRunSpec) -> Result<(), AgentHostError> {
        let docker = self.docker()?;
        let config = container_create_body(&spec);
        let response = if let Some(name) = spec.name.as_deref() {
            let options = CreateContainerOptionsBuilder::default().name(name).build();
            docker.create_container(Some(options), config).await?
        } else {
            docker
                .create_container(
                    None::<bollard::query_parameters::CreateContainerOptions>,
                    config,
                )
                .await?
        };
        let container_name = spec.name.as_deref().unwrap_or(&response.id);
        docker
            .start_container(
                container_name,
                None::<bollard::query_parameters::StartContainerOptions>,
            )
            .await?;
        let wait_result = if spec.detach {
            Ok(())
        } else {
            let options = WaitContainerOptionsBuilder::default()
                .condition("not-running")
                .build();
            let mut stream = docker.wait_container(container_name, Some(options));
            stream.next().await.map_or_else(
                || {
                    Err(AgentHostError::runtime(format!(
                        "Docker container {container_name:?} ended without an exit status"
                    )))
                },
                |result| {
                    let response = result.map_err(AgentHostError::from)?;
                    if response.status_code == 0 {
                        Ok(())
                    } else {
                        Err(AgentHostError::runtime(format!(
                            "Docker container {container_name:?} exited with status {}",
                            response.status_code
                        )))
                    }
                },
            )
        };
        let remove_result = if !spec.detach && spec.auto_remove {
            let options = RemoveContainerOptionsBuilder::default().force(true).build();
            match docker.remove_container(container_name, Some(options)).await {
                Ok(()) => Ok(()),
                Err(error) if is_bollard_not_found(&error) => Ok(()),
                Err(error) => Err(error.into()),
            }
        } else {
            Ok(())
        };
        wait_result.and(remove_result)
    }

    async fn inspect_container(
        &self,
        container_name: &str,
    ) -> Result<Option<DockerContainerResource>, AgentHostError> {
        let docker = self.docker()?;
        match docker
            .inspect_container(
                container_name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
        {
            Ok(inspect) => Ok(Some(container_resource(inspect, container_name))),
            Err(error) if is_bollard_not_found(&error) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn list_containers(&self) -> Result<Vec<DockerContainerResource>, AgentHostError> {
        let docker = self.docker()?;
        let options = ListContainersOptionsBuilder::default().all(true).build();
        let summaries = docker.list_containers(Some(options)).await?;
        let mut resources = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let identity = summary.id.or_else(|| {
                summary
                    .names
                    .and_then(|names| names.into_iter().next())
                    .map(|name| name.trim_start_matches('/').to_owned())
            });
            let Some(identity) = identity else {
                continue;
            };
            if let Some(resource) = self.inspect_container(&identity).await? {
                resources.push(resource);
            }
        }
        Ok(resources)
    }

    async fn inspect_volume(
        &self,
        volume_name: &str,
    ) -> Result<Option<DockerVolumeResource>, AgentHostError> {
        let docker = self.docker()?;
        match docker.inspect_volume(volume_name).await {
            Ok(volume) => Ok(Some(volume_resource(volume))),
            Err(error) if is_bollard_not_found(&error) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn list_volumes(&self) -> Result<Vec<DockerVolumeResource>, AgentHostError> {
        let docker = self.docker()?;
        let options = ListVolumesOptionsBuilder::default().build();
        let response = docker.list_volumes(Some(options)).await?;
        Ok(response
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(volume_resource)
            .collect())
    }

    async fn container_host_port(
        &self,
        container_name: &str,
        container_port: u16,
    ) -> Result<Option<u16>, AgentHostError> {
        let docker = self.docker()?;
        let inspect = match docker
            .inspect_container(
                container_name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
        {
            Ok(inspect) => inspect,
            Err(error) if is_bollard_not_found(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(extract_host_port(&inspect, container_port))
    }

    async fn container_is_running(&self, container_name: &str) -> Result<bool, AgentHostError> {
        Ok(self
            .inspect_container(container_name)
            .await?
            .is_some_and(|container| container.running))
    }

    async fn remove_container(&self, container_name: &str) -> Result<(), AgentHostError> {
        let docker = self.docker()?;
        let options = RemoveContainerOptionsBuilder::default().force(true).build();
        match docker.remove_container(container_name, Some(options)).await {
            Ok(()) => Ok(()),
            Err(error) if is_bollard_not_found(&error) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn remove_volume(&self, volume_name: &str) -> Result<(), AgentHostError> {
        let docker = self.docker()?;
        let options = RemoveVolumeOptionsBuilder::default().force(true).build();
        match docker.remove_volume(volume_name, Some(options)).await {
            Ok(()) => Ok(()),
            Err(error) if is_bollard_not_found(&error) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn container_logs(
        &self,
        container_name: &str,
        tail: Option<u32>,
    ) -> Result<Vec<String>, AgentHostError> {
        let docker = self.docker()?;
        let mut options = LogsOptionsBuilder::default().stdout(true).stderr(true);
        if let Some(tail) = tail {
            options = options.tail(&tail.to_string());
        }
        let mut stream = docker.logs(container_name, Some(options.build()));
        let mut raw = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(output) => append_log_output(&mut raw, &output),
                Err(error) if is_bollard_not_found(&error) => return Ok(Vec::new()),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(String::from_utf8_lossy(&raw)
            .lines()
            .map(ToOwned::to_owned)
            .collect())
    }

    async fn container_stats(
        &self,
        container_name: &str,
    ) -> Result<Option<DockerStats>, AgentHostError> {
        let docker = self.docker()?;
        let options = StatsOptionsBuilder::default()
            .stream(false)
            .one_shot(true)
            .build();
        let mut stream = docker.stats(container_name, Some(options));
        let Some(result) = stream.next().await else {
            return Ok(None);
        };
        match result {
            Ok(stats) => Ok(Some(serde_json::from_value(serde_json::to_value(stats)?)?)),
            Err(error) if is_bollard_not_found(&error) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn open_terminal_exec(
        &self,
        container_name: &str,
        attachment_id: &str,
        attach_argv: &[String],
    ) -> Result<TerminalExec, AgentHostError> {
        let docker = self.docker()?;
        let created = docker
            .create_exec(
                container_name,
                terminal_exec_options(attachment_id, attach_argv),
            )
            .await?;
        let started = docker
            .start_exec(
                &created.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: true,
                    output_capacity: Some(1024 * 1024),
                }),
            )
            .await?;
        let StartExecResults::Attached { output, input } = started else {
            return Err(AgentHostError::runtime(
                "Docker unexpectedly detached terminal exec",
            ));
        };

        let output = output.map(|item| item.map(log_output_bytes).map_err(AgentHostError::from));
        Ok(TerminalExec {
            exec_id: created.id,
            input,
            output: Box::pin(output),
        })
    }

    async fn resize_terminal_exec(
        &self,
        exec_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), AgentHostError> {
        self.docker()?
            .resize_exec(
                exec_id,
                ResizeExecOptions {
                    height: rows,
                    width: cols,
                },
            )
            .await?;
        Ok(())
    }
}

#[must_use]
pub fn summarize_docker_stats(raw: &DockerStats) -> Option<DockerStatsSummary> {
    let cpu_percent = compute_cpu_percent(&raw.cpu, &raw.precpu);
    let memory_usage = compute_memory_usage(&raw.memory);
    let memory_limit = raw.memory.limit;
    let memory_percent = memory_usage
        .zip(memory_limit)
        .and_then(|(usage, limit)| (limit > 0).then_some(percent(usage, limit, 1)));

    if cpu_percent.is_none() && memory_usage.is_none() && memory_limit.is_none() {
        return None;
    }

    Some(DockerStatsSummary {
        cpu_percent,
        memory_usage_bytes: memory_usage,
        memory_limit_bytes: memory_limit,
        memory_percent,
    })
}

fn compute_cpu_percent(cpu_stats: &DockerCpuStats, precpu_stats: &DockerCpuStats) -> Option<f64> {
    let total = cpu_stats.cpu_usage.total_usage?;
    let pre_total = precpu_stats.cpu_usage.total_usage?;
    let system = cpu_stats.system_cpu_usage?;
    let pre_system = precpu_stats.system_cpu_usage?;
    let cpu_delta = total.checked_sub(pre_total)?;
    let system_delta = system.checked_sub(pre_system)?;
    if system_delta == 0 {
        return None;
    }
    let online_cpus = cpu_stats
        .online_cpus
        .filter(|value| *value > 0)
        .or_else(|| {
            (!cpu_stats.cpu_usage.percpu_usage.is_empty())
                .then(|| u64::try_from(cpu_stats.cpu_usage.percpu_usage.len()).unwrap_or(1))
                .filter(|value| *value > 0)
        })
        .unwrap_or(1);
    Some(percent(cpu_delta, system_delta, online_cpus))
}

#[allow(clippy::cast_precision_loss)]
fn percent(numerator: u64, denominator: u64, multiplier: u64) -> f64 {
    (numerator as f64 / denominator as f64) * multiplier as f64 * 100.0
}

fn compute_memory_usage(memory_stats: &DockerMemoryStats) -> Option<u64> {
    let usage = memory_stats.usage?;
    let cache = memory_stats
        .stats
        .cache
        .or(memory_stats.stats.inactive_file);
    match cache {
        Some(cache) if cache <= usage => Some(usage - cache),
        _ => Some(usage),
    }
}

fn parse_event_line(line: &[u8]) -> Result<Option<KernelEvent>, AgentHostError> {
    let trimmed = trim_ascii_whitespace(line);
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(trimmed)?))
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while matches!(bytes.first(), Some(byte) if byte.is_ascii_whitespace()) {
        bytes = &bytes[1..];
    }
    while matches!(bytes.last(), Some(byte) if byte.is_ascii_whitespace()) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn normalize_runtime_summary(mut summary: RuntimeSessionSummary) -> RuntimeSessionSummary {
    summary.resume_token = non_empty(summary.resume_token);
    summary.vscode_url = non_empty(summary.vscode_url);
    summary.free_port_url = non_empty(summary.free_port_url);
    summary
}

fn extract_host_port(
    inspect: &bollard::models::ContainerInspectResponse,
    container_port: u16,
) -> Option<u16> {
    let port_key = format!("{container_port}/tcp");
    inspect
        .network_settings
        .as_ref()?
        .ports
        .as_ref()?
        .get(&port_key)?
        .as_ref()?
        .first()?
        .host_port
        .as_ref()?
        .parse()
        .ok()
}

fn container_create_body(spec: &ContainerRunSpec) -> ContainerCreateBody {
    let (entrypoint, cmd) = spec
        .entrypoint
        .split_first()
        .map_or((None, None), |(first, args)| {
            (
                Some(vec![first.clone()]),
                (!args.is_empty()).then(|| args.to_vec()),
            )
        });

    ContainerCreateBody {
        image: Some(spec.image.clone()),
        env: Some(environment_entries(&spec.environment)),
        entrypoint,
        cmd,
        labels: Some(hash_map_from_btree(&spec.labels)),
        exposed_ports: docker_exposed_ports(&spec.ports),
        network_disabled: spec.network_disabled.then_some(true),
        host_config: Some(HostConfig {
            auto_remove: Some(spec.auto_remove && spec.detach),
            network_mode: container_network_mode(spec),
            port_bindings: docker_port_bindings(&spec.ports),
            binds: docker_volume_binds(&spec.volumes),
            ..HostConfig::default()
        }),
        ..ContainerCreateBody::default()
    }
}

fn container_network_mode(spec: &ContainerRunSpec) -> Option<String> {
    if spec.network_disabled {
        Some("none".to_owned())
    } else {
        spec.network.clone()
    }
}

fn docker_port_bindings(ports: &[PortBinding]) -> Option<PortMap> {
    if ports.is_empty() {
        return None;
    }
    let mut bindings = PortMap::new();
    for port in ports {
        let entry = bindings
            .entry(container_port_key(port.container_port))
            .or_insert_with(|| Some(Vec::new()));
        if let Some(values) = entry {
            values.push(DockerPortBinding {
                host_ip: Some(port.host_ip.clone()),
                host_port: Some(String::new()),
            });
        }
    }
    Some(bindings)
}

fn docker_exposed_ports(ports: &[PortBinding]) -> Option<Vec<String>> {
    if ports.is_empty() {
        return None;
    }
    Some(
        ports
            .iter()
            .map(|port| container_port_key(port.container_port))
            .collect(),
    )
}

fn docker_volume_binds(volumes: &[VolumeMount]) -> Option<Vec<String>> {
    if volumes.is_empty() {
        return None;
    }
    Some(
        volumes
            .iter()
            .map(|volume| format!("{}:{}:{}", volume.volume_name, volume.bind, volume.mode))
            .collect(),
    )
}

fn container_port_key(container_port: u16) -> String {
    format!("{container_port}/tcp")
}

fn environment_entries(environment: &BTreeMap<String, String>) -> Vec<String> {
    environment
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

fn container_resource(
    inspect: bollard::models::ContainerInspectResponse,
    fallback_name: &str,
) -> DockerContainerResource {
    let resource_id = inspect.id.clone().unwrap_or_default();
    let name = inspect
        .name
        .unwrap_or_else(|| fallback_name.to_owned())
        .trim_start_matches('/')
        .to_owned();
    let labels = inspect
        .config
        .and_then(|config| config.labels)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let (running, status) = inspect.state.map_or_else(
        || (false, "unknown".to_owned()),
        |state| {
            let running = state.running.unwrap_or(false);
            let status = state
                .status
                .and_then(|status| serde_json::to_value(status).ok())
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| {
                    if running {
                        "running".to_owned()
                    } else {
                        "unknown".to_owned()
                    }
                });
            (running, status)
        },
    );
    let mounts = inspect
        .mounts
        .unwrap_or_default()
        .into_iter()
        .filter_map(|mount| Some((mount.destination?, mount.name?)))
        .collect();
    DockerContainerResource {
        name,
        resource_id,
        labels,
        running,
        status,
        mounts,
    }
}

fn volume_resource(volume: bollard::models::Volume) -> DockerVolumeResource {
    let resource_id = volume
        .labels
        .get(LABEL_RESOURCE_ID)
        .cloned()
        .or(volume.created_at)
        .unwrap_or_default();
    DockerVolumeResource {
        name: volume.name,
        resource_id,
        labels: volume.labels.into_iter().collect(),
    }
}

fn kernel_labels(request: &RuntimeCreateSession) -> BTreeMap<String, String> {
    btree_map([
        (LABEL_ROLE, "kernel"),
        (LABEL_MANAGED, "true"),
        (LABEL_SESSION_ID, request.session_id.as_str()),
        (LABEL_INTERACTION_MODE, request.interaction_mode.as_str()),
        ("agentspace.harness", request.harness.as_str()),
    ])
}

fn session_workspace_labels(request: &RuntimeCreateSession) -> BTreeMap<String, String> {
    btree_map([
        (LABEL_ROLE, "session-workspace"),
        (LABEL_MANAGED, "true"),
        (LABEL_SESSION_ID, request.session_id.as_str()),
        (LABEL_INTERACTION_MODE, request.interaction_mode.as_str()),
    ])
}

fn session_telemetry_labels(request: &RuntimeCreateSession) -> BTreeMap<String, String> {
    btree_map([
        (LABEL_ROLE, "session-telemetry"),
        (LABEL_MANAGED, "true"),
        (LABEL_SESSION_ID, request.session_id.as_str()),
    ])
}

fn recovery_expected(request: &RuntimeCreateSession) -> bool {
    request.env.get(RUNTIME_RECOVERY_ENV).map(String::as_str) == Some("1")
}

fn validate_session_resource_labels(
    resource_kind: &str,
    resource_name: &str,
    labels: &BTreeMap<String, String>,
    expected_role: &str,
    interaction_mode: InteractionMode,
) -> Result<(), AgentHostError> {
    if !is_managed_role(labels, expected_role)
        || labels.get(LABEL_INTERACTION_MODE).map(String::as_str) != Some(interaction_mode.as_str())
    {
        return Err(AgentHostError::conflict(format!(
            "{resource_kind} {resource_name:?} has incompatible AgentSpace ownership labels"
        )));
    }
    Ok(())
}

fn validate_telemetry_volume_labels(
    resource_kind: &str,
    resource_name: &str,
    labels: &BTreeMap<String, String>,
) -> Result<(), AgentHostError> {
    if !is_managed_role(labels, "session-telemetry")
        || labels.get(LABEL_SESSION_ID).is_none()
        || labels.get(LABEL_INTERACTION_MODE).is_some()
        || labels.get(LABEL_RESOURCE_ID).is_some()
        || labels.len() != 3
    {
        return Err(AgentHostError::conflict(format!(
            "{resource_kind} {resource_name:?} has incompatible AgentSpace ownership labels"
        )));
    }
    Ok(())
}

fn is_managed_role(labels: &BTreeMap<String, String>, role: &str) -> bool {
    labels.get(LABEL_MANAGED).map(String::as_str) == Some("true")
        && labels.get(LABEL_ROLE).map(String::as_str) == Some(role)
}

fn resource_is_owned(
    labels: &BTreeMap<String, String>,
    owned_session_ids: &BTreeSet<String>,
) -> bool {
    labels
        .get(LABEL_SESSION_ID)
        .is_some_and(|session_id| owned_session_ids.contains(session_id))
}

fn identity_collision(
    resource_kind: &str,
    resource_name: &str,
    session_id: &str,
) -> AgentHostError {
    AgentHostError::conflict(format!(
        "Docker {resource_kind} {resource_name:?} is not owned by session {session_id:?}"
    ))
}

fn validate_reviewed_container(
    current: Option<&DockerContainerResource>,
    reviewed: &CleanupResourceIdentity,
) -> Result<(), AgentHostError> {
    let current = current.ok_or_else(|| {
        AgentHostError::conflict(format!(
            "reviewed Docker container {:?} no longer exists",
            reviewed.name
        ))
    })?;
    validate_reviewed_resource(
        "container",
        &current.resource_id,
        &current.labels,
        reviewed,
        "kernel",
    )
}

fn validate_reviewed_volume(
    current: Option<&DockerVolumeResource>,
    reviewed: &CleanupResourceIdentity,
    kind: CleanupResourceKind,
) -> Result<(), AgentHostError> {
    let current = current.ok_or_else(|| {
        AgentHostError::conflict(format!(
            "reviewed Docker volume {:?} no longer exists",
            reviewed.name
        ))
    })?;
    let expected_role = match kind {
        CleanupResourceKind::SessionWorkspaceVolume => "session-workspace",
        CleanupResourceKind::SessionTelemetryVolume => "session-telemetry",
        CleanupResourceKind::KernelContainer => {
            return Err(AgentHostError::runtime(
                "volume cleanup validation received a container resource",
            ));
        }
    };
    validate_reviewed_resource(
        "volume",
        &current.resource_id,
        &current.labels,
        reviewed,
        expected_role,
    )
}

fn validate_reviewed_resource(
    resource_kind: &str,
    current_resource_id: &str,
    labels: &BTreeMap<String, String>,
    reviewed: &CleanupResourceIdentity,
    expected_role: &str,
) -> Result<(), AgentHostError> {
    if reviewed.resource_id.is_empty()
        || current_resource_id != reviewed.resource_id
        || !is_managed_role(labels, expected_role)
        || labels.get(LABEL_SESSION_ID) != reviewed.session_id.as_ref()
    {
        return Err(AgentHostError::conflict(format!(
            "reviewed Docker {resource_kind} {:?} changed identity or ownership after review",
            reviewed.name
        )));
    }
    Ok(())
}

fn cleanup_resource(
    kind: CleanupResourceKind,
    name: String,
    resource_id: String,
    labels: &BTreeMap<String, String>,
    status: Option<String>,
    dry_run: bool,
    result: Result<(), AgentHostError>,
) -> CleanupResource {
    let (action, error) = match result {
        Ok(()) if dry_run => (CleanupAction::WouldDelete, None),
        Ok(()) => (CleanupAction::Deleted, None),
        Err(error) => (CleanupAction::DeleteFailed, Some(error.to_string())),
    };
    CleanupResource {
        kind,
        name,
        resource_id,
        session_id: labels.get(LABEL_SESSION_ID).cloned(),
        interaction_mode: labels.get(LABEL_INTERACTION_MODE).cloned(),
        status,
        action,
        error,
    }
}

fn hash_map_from_btree(map: &BTreeMap<String, String>) -> HashMap<String, String> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn append_log_output(buffer: &mut Vec<u8>, output: &LogOutput) {
    buffer.extend_from_slice(output.as_ref());
}

fn log_output_bytes(output: LogOutput) -> axum::body::Bytes {
    match output {
        LogOutput::StdErr { message }
        | LogOutput::StdOut { message }
        | LogOutput::StdIn { message }
        | LogOutput::Console { message } => message,
    }
}

fn terminal_exec_options(attachment_id: &str, attach_argv: &[String]) -> CreateExecOptions<String> {
    CreateExecOptions {
        attach_stdin: Some(true),
        attach_stdout: Some(true),
        attach_stderr: Some(true),
        tty: Some(true),
        env: Some(vec![format!("{ATTACHMENT_ID_ENV}={attachment_id}")]),
        cmd: Some(attach_argv.to_vec()),
        ..CreateExecOptions::default()
    }
}

const fn is_bollard_not_found(error: &BollardError) -> bool {
    matches!(
        error,
        BollardError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn btree_map<const N: usize>(entries: [(&str, &str); N]) -> BTreeMap<String, String> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn env_u16(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

const fn path_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

const fn skills_mount_path(harness: HarnessName) -> &'static str {
    match harness {
        HarnessName::Acp => "/workspace/.agents/skills",
        HarnessName::ClaudeCode | HarnessName::Codex | HarnessName::Echo => "/skills",
        HarnessName::CopilotCli => "/workspace/.github/skills",
        HarnessName::Opencode => "/root/.config/opencode/skills",
    }
}

fn vscode_enabled(value: Option<&String>) -> bool {
    value.is_none_or(|value| {
        !matches!(
            value.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

fn kernel_entrypoint() -> Vec<String> {
    [
        "/usr/local/bin/uv",
        "run",
        "--no-dev",
        "--package",
        "kernel-host",
        "-m",
        "kernel_host.api_main",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect()
}

fn kernel_container_name(session_id: &str) -> String {
    format!("agentspace-kernel-{}", first_n(session_id, 12))
}

fn session_workspace_volume_name(session_id: &str) -> String {
    format!("agentspace-session-workspace-{}", first_n(session_id, 12))
}

fn session_telemetry_volume_name(volume_identity: &str) -> String {
    format!(
        "agentspace-session-telemetry-{}",
        first_n(volume_identity, 12)
    )
}

#[cfg(test)]
fn session_workspace_volume_name_from_container(
    container_name: &str,
) -> Result<String, AgentHostError> {
    let Some(suffix) = container_name.strip_prefix("agentspace-kernel-") else {
        return Err(AgentHostError::runtime(format!(
            "unexpected kernel container name {container_name:?}"
        )));
    };
    Ok(format!("agentspace-session-workspace-{suffix}"))
}

fn workspace_editor_container_name(workspace_id: &str) -> String {
    format!("agentspace-workspace-editor-{workspace_id}")
}

fn first_n(value: &str, count: usize) -> &str {
    value.get(..count).unwrap_or(value)
}

fn format_runtime_template(
    template: &str,
    container_name: &str,
    host_ip: &str,
    host_port: u16,
    container_port: u16,
) -> String {
    template
        .replace(CONTAINER_NAME_PLACEHOLDER, container_name)
        .replace(HOST_IP_PLACEHOLDER, host_ip)
        .replace(HOST_PORT_PLACEHOLDER, &host_port.to_string())
        .replace(CONTAINER_PORT_PLACEHOLDER, &container_port.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::{Arc, Mutex, MutexGuard, PoisonError},
        time::Duration,
    };

    use async_trait::async_trait;
    use axum::{
        Json, Router,
        body::to_bytes,
        extract::State,
        http::{Request, StatusCode},
        routing::{get, post},
    };
    use futures_util::stream;
    use serde_json::{Value as JsonValue, json};
    use tokio::{net::TcpListener, task::JoinHandle};

    use super::{
        ContainerRunSpec, DockerBackend, DockerContainerResource, DockerKernelRuntime,
        DockerRuntimeConfig, DockerStats, DockerVolumeResource, PortBinding, RUNTIME_RECOVERY_ENV,
        SESSION_TELEMETRY_MOUNT_PATH, SESSION_WORKSPACE_MOUNT_PATH, TERMINAL_RESUME_ENV,
        VolumeMount, btree_map, container_create_body, session_telemetry_labels,
        session_telemetry_volume_name, session_workspace_labels,
        session_workspace_volume_name_from_container, summarize_docker_stats,
        terminal_exec_options,
    };
    use crate::{
        docker_runtime::skills_mount_path,
        errors::AgentHostError,
        models::{
            CacheSignalState, CleanupAction, CleanupResourceIdentity, DockerKernelSession,
            HarnessName, InteractionMode, KernelRuntimeSession, TelemetryContentMode,
            TelemetryState, WorkspaceMount, WorkspaceMountMode,
        },
        sessions::{KernelRuntime, RuntimeCreateSession},
        skills::{SkillVolumeMode, SkillVolumeResource},
        terminal::{TerminalAttachKind, TerminalExec, TerminalState},
    };

    type KernelHttpCalls = Arc<Mutex<Vec<(String, JsonValue)>>>;

    #[derive(Clone, Default)]
    struct FakeDockerBackend {
        state: Arc<Mutex<FakeDockerState>>,
    }

    #[derive(Default)]
    struct FakeDockerState {
        volumes: BTreeSet<String>,
        volume_labels: BTreeMap<String, BTreeMap<String, String>>,
        created_volumes: Vec<(String, BTreeMap<String, String>)>,
        containers: BTreeMap<String, DockerContainerResource>,
        run_specs: Vec<ContainerRunSpec>,
        removed_containers: Vec<String>,
        removed_volumes: Vec<String>,
        running: BTreeSet<String>,
        ports: BTreeMap<(String, u16), u16>,
        terminal_attach_argvs: Vec<(String, Vec<String>)>,
        terminal_resizes: Vec<(String, u16, u16)>,
        fail_next_run: bool,
    }

    impl FakeDockerBackend {
        fn state(&self) -> MutexGuard<'_, FakeDockerState> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    async fn runtime_with_health(
        backend: &FakeDockerBackend,
    ) -> (DockerKernelRuntime, JoinHandle<Result<(), std::io::Error>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("failed to bind health server: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("failed to read health address: {error}"));
        let app = Router::new().route("/healthz", get(|| async { StatusCode::OK }));
        let handle = tokio::spawn(async move { axum::serve(listener, app).await });
        let config = DockerRuntimeConfig {
            base_url_template: format!("http://{address}"),
            startup_timeout: Duration::from_secs(2),
            ..DockerRuntimeConfig::default()
        };
        (
            DockerKernelRuntime::new(config, Arc::new(backend.clone())),
            handle,
        )
    }

    fn runtime_request(session_id: &str) -> RuntimeCreateSession {
        RuntimeCreateSession {
            session_id: session_id.to_owned(),
            telemetry_volume_identity: None,
            harness: HarnessName::Echo,
            interaction_mode: InteractionMode::Chat,
            env: BTreeMap::new(),
            additional_paths: Vec::new(),
            skills: Vec::new(),
            skill_volumes: Vec::new(),
            workspace_mounts: Vec::new(),
        }
    }

    fn runtime_request_with_telemetry(session_id: &str) -> RuntimeCreateSession {
        let mut request = runtime_request(session_id);
        request.telemetry_volume_identity = Some(session_id.to_owned());
        request
    }

    fn seed_volume(state: &mut FakeDockerState, name: &str, labels: BTreeMap<String, String>) {
        state.volumes.insert(name.to_owned());
        state.volume_labels.insert(name.to_owned(), labels);
    }

    fn seed_container_with_mounts(
        state: &mut FakeDockerState,
        name: &str,
        labels: BTreeMap<String, String>,
        running: bool,
        status: &str,
        mounts: BTreeMap<String, String>,
    ) {
        state.containers.insert(
            name.to_owned(),
            DockerContainerResource {
                name: name.to_owned(),
                resource_id: format!("container:{name}"),
                labels,
                running,
                status: status.to_owned(),
                mounts,
            },
        );
        if running {
            state.running.insert(name.to_owned());
        }
    }

    fn seed_container(
        state: &mut FakeDockerState,
        name: &str,
        labels: BTreeMap<String, String>,
        running: bool,
        status: &str,
        workspace_volume: Option<&str>,
    ) {
        seed_container_with_mounts(
            state,
            name,
            labels,
            running,
            status,
            workspace_volume.map_or_else(BTreeMap::new, |volume| {
                BTreeMap::from([(SESSION_WORKSPACE_MOUNT_PATH.to_owned(), volume.to_owned())])
            }),
        );
    }

    #[async_trait]
    impl DockerBackend for FakeDockerBackend {
        async fn ensure_volume(
            &self,
            volume_name: &str,
            labels: BTreeMap<String, String>,
        ) -> Result<(), AgentHostError> {
            {
                let mut state = self.state();
                if state.volumes.insert(volume_name.to_owned()) {
                    state
                        .volume_labels
                        .insert(volume_name.to_owned(), labels.clone());
                    state.created_volumes.push((volume_name.to_owned(), labels));
                }
            }
            Ok(())
        }

        async fn require_volume(&self, volume_name: &str) -> Result<(), AgentHostError> {
            if self.state().volumes.contains(volume_name) {
                Ok(())
            } else {
                Err(AgentHostError::runtime("missing volume"))
            }
        }

        async fn run_container(&self, spec: ContainerRunSpec) -> Result<(), AgentHostError> {
            {
                let mut state = self.state();
                if state.fail_next_run {
                    state.fail_next_run = false;
                    return Err(AgentHostError::runtime(
                        "Docker helper container exited with status 1",
                    ));
                }
                if let Some(name) = &spec.name {
                    state.running.insert(name.clone());
                    state.ports.insert((name.clone(), 8080), 45_678);
                    state.ports.insert((name.clone(), 8081), 45_679);
                    state.containers.insert(
                        name.clone(),
                        DockerContainerResource {
                            name: name.clone(),
                            resource_id: format!("container:{name}"),
                            labels: spec.labels.clone(),
                            running: true,
                            status: "running".to_owned(),
                            mounts: spec
                                .volumes
                                .iter()
                                .map(|volume| (volume.bind.clone(), volume.volume_name.clone()))
                                .collect(),
                        },
                    );
                }
                state.run_specs.push(spec);
            }
            Ok(())
        }

        async fn inspect_container(
            &self,
            container_name: &str,
        ) -> Result<Option<DockerContainerResource>, AgentHostError> {
            Ok(self.state().containers.get(container_name).cloned())
        }

        async fn list_containers(&self) -> Result<Vec<DockerContainerResource>, AgentHostError> {
            Ok(self.state().containers.values().cloned().collect())
        }

        async fn inspect_volume(
            &self,
            volume_name: &str,
        ) -> Result<Option<DockerVolumeResource>, AgentHostError> {
            Ok(self
                .state()
                .volume_labels
                .get(volume_name)
                .map(|labels| DockerVolumeResource {
                    name: volume_name.to_owned(),
                    resource_id: format!("volume:{volume_name}"),
                    labels: labels.clone(),
                }))
        }

        async fn list_volumes(&self) -> Result<Vec<DockerVolumeResource>, AgentHostError> {
            Ok(self
                .state()
                .volume_labels
                .iter()
                .map(|(name, labels)| DockerVolumeResource {
                    name: name.clone(),
                    resource_id: format!("volume:{name}"),
                    labels: labels.clone(),
                })
                .collect())
        }

        async fn container_host_port(
            &self,
            container_name: &str,
            container_port: u16,
        ) -> Result<Option<u16>, AgentHostError> {
            Ok(self
                .state()
                .ports
                .get(&(container_name.to_owned(), container_port))
                .copied())
        }

        async fn container_is_running(&self, container_name: &str) -> Result<bool, AgentHostError> {
            Ok(self.state().running.contains(container_name))
        }

        async fn remove_container(&self, container_name: &str) -> Result<(), AgentHostError> {
            let mut state = self.state();
            state.removed_containers.push(container_name.to_owned());
            state.running.remove(container_name);
            state.containers.remove(container_name);
            drop(state);
            Ok(())
        }

        async fn remove_volume(&self, volume_name: &str) -> Result<(), AgentHostError> {
            let mut state = self.state();
            state.removed_volumes.push(volume_name.to_owned());
            state.volumes.remove(volume_name);
            state.volume_labels.remove(volume_name);
            drop(state);
            Ok(())
        }

        async fn container_logs(
            &self,
            _container_name: &str,
            _tail: Option<u32>,
        ) -> Result<Vec<String>, AgentHostError> {
            Ok(Vec::new())
        }

        async fn container_stats(
            &self,
            _container_name: &str,
        ) -> Result<Option<DockerStats>, AgentHostError> {
            Ok(None)
        }

        async fn open_terminal_exec(
            &self,
            _container_name: &str,
            attachment_id: &str,
            attach_argv: &[String],
        ) -> Result<TerminalExec, AgentHostError> {
            self.state()
                .terminal_attach_argvs
                .push((attachment_id.to_owned(), attach_argv.to_vec()));
            Ok(TerminalExec {
                exec_id: "exec-test".to_owned(),
                input: Box::pin(tokio::io::sink()),
                output: Box::pin(stream::empty()),
            })
        }

        async fn resize_terminal_exec(
            &self,
            exec_id: &str,
            cols: u16,
            rows: u16,
        ) -> Result<(), AgentHostError> {
            self.state()
                .terminal_resizes
                .push((exec_id.to_owned(), cols, rows));
            Ok(())
        }
    }

    #[tokio::test]
    async fn docker_runtime_delegates_fixed_terminal_exec_and_resize() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        let session = KernelRuntimeSession::Docker(DockerKernelSession {
            session_id: "session".to_owned(),
            container_name: "kernel".to_owned(),
            session_workspace_volume_name: "workspace".to_owned(),
            session_telemetry_volume_name: None,
            base_url: "http://kernel".to_owned(),
            vscode_url: None,
            free_port_url: None,
        });
        let argv = vec![
            "tmux".to_owned(),
            "attach-session".to_owned(),
            "-t".to_owned(),
            "=agentspace-test".to_owned(),
        ];

        let exec = runtime
            .terminal_attach(&session, "22222222-2222-4222-8222-222222222222", &argv)
            .await
            .unwrap_or_else(|error| panic!("terminal attach failed: {error}"));
        runtime
            .terminal_resize(&session, &exec.exec_id, 120, 40)
            .await
            .unwrap_or_else(|error| panic!("terminal resize failed: {error}"));
        drop(exec);

        {
            let state = backend.state();
            assert_eq!(
                state.terminal_attach_argvs,
                vec![("22222222-2222-4222-8222-222222222222".to_owned(), argv)]
            );
            assert_eq!(
                state.terminal_resizes,
                vec![("exec-test".to_owned(), 120, 40)]
            );
            drop(state);
        }
    }

    #[test]
    fn terminal_exec_options_use_tty_and_attachment_marker() {
        let argv = vec![
            "tmux".to_owned(),
            "attach-session".to_owned(),
            "-t".to_owned(),
            "=agentspace-test".to_owned(),
        ];

        let options = terminal_exec_options("22222222-2222-4222-8222-222222222222", &argv);

        assert_eq!(options.attach_stdin, Some(true));
        assert_eq!(options.attach_stdout, Some(true));
        assert_eq!(options.attach_stderr, Some(true));
        assert_eq!(options.tty, Some(true));
        assert_eq!(options.cmd, Some(argv));
        assert_eq!(
            options.env,
            Some(vec![
                "AGENTSPACE_TERMINAL_ATTACHMENT_ID=22222222-2222-4222-8222-222222222222".to_owned()
            ])
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn docker_runtime_proxies_structured_terminal_controls_and_telemetry() {
        let calls: KernelHttpCalls = Arc::new(Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("failed to bind terminal server: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("failed to read terminal address: {error}"));
        let app = Router::new()
            .route("/terminal", get(terminal_proxy_stub))
            .route("/terminal/ensure", post(terminal_proxy_stub))
            .route("/terminal/stop", post(terminal_proxy_stub))
            .route("/terminal/resume", post(terminal_proxy_stub))
            .route("/terminal/detach-client", post(terminal_proxy_stub))
            .route("/telemetry", get(telemetry_proxy_stub))
            .with_state(calls.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let runtime = DockerKernelRuntime::new(
            DockerRuntimeConfig::default(),
            Arc::new(FakeDockerBackend::default()),
        );
        let session = KernelRuntimeSession::Docker(DockerKernelSession {
            session_id: "session".to_owned(),
            container_name: "kernel".to_owned(),
            session_workspace_volume_name: "workspace".to_owned(),
            session_telemetry_volume_name: None,
            base_url: format!("http://{address}"),
            vscode_url: None,
            free_port_url: None,
        });

        assert_eq!(
            runtime
                .terminal_status(&session)
                .await
                .unwrap_or_else(|error| panic!("status failed: {error}"))
                .state,
            TerminalState::Running
        );
        assert_eq!(
            runtime
                .terminal_ensure(&session)
                .await
                .unwrap_or_else(|error| panic!("ensure failed: {error}"))
                .attach_kind,
            Some(TerminalAttachKind::Attached)
        );
        runtime
            .terminal_stop(&session)
            .await
            .unwrap_or_else(|error| panic!("stop failed: {error}"));
        runtime
            .terminal_resume(&session)
            .await
            .unwrap_or_else(|error| panic!("resume failed: {error}"));
        runtime
            .terminal_detach_client(&session, "/dev/pts/7")
            .await
            .unwrap_or_else(|error| panic!("detach failed: {error}"));
        let telemetry = runtime
            .telemetry(&session)
            .await
            .unwrap_or_else(|error| panic!("telemetry failed: {error}"));

        let calls = calls.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            calls
                .iter()
                .map(|(path, _body)| path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "/terminal",
                "/terminal/ensure",
                "/terminal/stop",
                "/terminal/resume",
                "/terminal/detach-client",
                "/telemetry",
            ]
        );
        assert_eq!(calls[4].1, json!({ "tmux_client_id": "/dev/pts/7" }));
        assert_eq!(calls[5].1, JsonValue::Null);
        assert_eq!(telemetry.state, TelemetryState::Live);
        assert_eq!(telemetry.content_mode, TelemetryContentMode::Metadata);
        assert_eq!(
            telemetry
                .latest_call
                .as_ref()
                .and_then(|call| call.model.as_deref()),
            Some("gpt-5.6-sol")
        );
        assert_eq!(
            telemetry.cache_signal.as_ref().map(|signal| signal.state),
            Some(CacheSignalState::CacheResetSuspected)
        );
        drop(calls);
        server.abort();
    }

    async fn terminal_proxy_stub(
        State(calls): State<KernelHttpCalls>,
        request: Request<axum::body::Body>,
    ) -> Json<JsonValue> {
        let path = request.uri().path().to_owned();
        let bytes = to_bytes(request.into_body(), 16 * 1024)
            .await
            .unwrap_or_else(|error| panic!("failed to read terminal request: {error}"));
        let body = if bytes.is_empty() {
            JsonValue::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("failed to parse terminal request: {error}"))
        };
        calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((path, body));
        Json(terminal_proxy_payload())
    }

    async fn telemetry_proxy_stub(
        State(calls): State<KernelHttpCalls>,
        request: Request<axum::body::Body>,
    ) -> Json<JsonValue> {
        let path = request.uri().path().to_owned();
        let bytes = to_bytes(request.into_body(), 16 * 1024)
            .await
            .unwrap_or_else(|error| panic!("failed to read telemetry request: {error}"));
        let body = if bytes.is_empty() {
            JsonValue::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("failed to parse telemetry request: {error}"))
        };
        calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((path, body));
        Json(telemetry_proxy_payload())
    }

    fn terminal_proxy_payload() -> JsonValue {
        json!({
            "state": "running",
            "exit_status": null,
            "attach_kind": "attached",
            "session_name": "agentspace-test",
            "target_session": "=agentspace-test",
            "socket_path": "/run/agentspace-tmux.sock",
            "attach_argv": ["tmux", "attach-session", "-t", "=agentspace-test"],
            "pane_id": "%0",
            "pane_pid": 42,
            "attachment_count": 0,
            "clients": [],
        })
    }

    #[allow(clippy::too_many_lines)]
    fn telemetry_proxy_payload() -> JsonValue {
        json!({
            "schema_version": 1,
            "state": "live",
            "reason": null,
            "content_mode": "metadata",
            "source_version": "1.0.81-0",
            "observed_at": "2026-08-15T00:00:00Z",
            "received_at": "2026-08-15T00:00:01Z",
            "session": {
                "raw_input_tokens": 12,
                "effective_input_tokens": 9,
                "output_tokens": 3,
                "total_tokens": 15,
                "reasoning_output_tokens": 1,
                "cache_read_input_tokens": 2,
                "cache_write_input_tokens": 1,
                "other_input_tokens": 5,
                "fresh_input_tokens": 7,
                "cache_reuse_percent": 22.5,
                "nano_aiu": 8,
                "opaque_cost": 0.5
            },
            "latest_call": {
                "started_at": "2026-08-15T00:00:00Z",
                "ended_at": "2026-08-15T00:00:01Z",
                "duration_ms": 1000,
                "model": "gpt-5.6-sol",
                "requested_model": "gpt-5.6-sol",
                "provider": "openai",
                "agent_id": "builtin:task",
                "agent_name": "task",
                "is_subagent": true,
                "cache_reporting": "reported",
                "token_accounting_convention": "inclusive",
                "usage": {
                    "raw_input_tokens": 6,
                    "effective_input_tokens": 4,
                    "output_tokens": 2,
                    "total_tokens": 8,
                    "reasoning_output_tokens": 1,
                    "cache_read_input_tokens": 2,
                    "cache_write_input_tokens": 1,
                    "other_input_tokens": 1,
                    "fresh_input_tokens": 3,
                    "cache_reuse_percent": 33.3,
                    "nano_aiu": 4,
                    "opaque_cost": 0.25
                }
            },
            "last_interaction": {
                "raw_input_tokens": 10,
                "effective_input_tokens": 8,
                "output_tokens": 3,
                "total_tokens": 13,
                "reasoning_output_tokens": 1,
                "cache_read_input_tokens": 2,
                "cache_write_input_tokens": 1,
                "other_input_tokens": 5,
                "fresh_input_tokens": 6,
                "cache_reuse_percent": 20.0,
                "nano_aiu": 6,
                "opaque_cost": 0.4
            },
            "context": {
                "tokens": 111,
                "limit": 222,
                "message_count": 3,
                "observed_at": "2026-08-15T00:00:00Z"
            },
            "counts": {
                "interactions": 1,
                "model_calls": 2,
                "tool_calls": 3,
                "subagent_invocations": 4,
                "subagent_model_calls": 5,
                "errors": 6
            },
            "subagents": {
                "invocations": 1,
                "model_calls": 2,
                "effective_input_tokens": 3,
                "output_tokens": 4,
                "cache_read_input_tokens": 5,
                "cache_write_input_tokens": 6,
                "duration_ms": 7
            },
            "cache_signal": {
                "state": "cache_reset_suspected",
                "confidence": "medium",
                "reason": "context_discontinuity"
            },
            "reporting": {
                "model_calls": 2,
                "cache_reported_calls": 1,
                "convention_resolved_calls": 2,
                "effective_input_covered_calls": 2,
                "context_reported": true
            },
            "warnings": {
                "total": 2,
                "items": [
                    {
                        "code": "malformed_record",
                        "count": 2
                    }
                ]
            }
        })
    }

    #[tokio::test]
    async fn docker_runtime_builds_kernel_container_spec() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        let request = RuntimeCreateSession {
            session_id: "test".to_owned(),
            telemetry_volume_identity: Some("test".to_owned()),
            harness: HarnessName::Echo,
            interaction_mode: InteractionMode::Chat,
            env: BTreeMap::new(),
            additional_paths: Vec::new(),
            skills: Vec::new(),
            skill_volumes: Vec::new(),
            workspace_mounts: vec![
                WorkspaceMount::new("todo-list-code", WorkspaceMountMode::ReadWrite),
                WorkspaceMount::new("todo-list-items", WorkspaceMountMode::ReadOnly),
            ],
        };

        runtime
            .run_kernel_container(
                "agentspace-kernel-test",
                "agentspace-session-workspace-test",
                Some("agentspace-session-telemetry-test"),
                &request,
            )
            .await
            .unwrap_or_else(|error| panic!("run failed: {error}"));

        {
            let state = backend.state();
            let spec = &state.run_specs[0];
            assert_eq!(spec.image, "agentspace-kernel-kernel:latest");
            assert!(spec.auto_remove);
            assert!(spec.detach);
            assert_eq!(spec.name.as_deref(), Some("agentspace-kernel-test"));
            assert_eq!(spec.environment["KERNEL_HARNESS"], "echo");
            assert_eq!(spec.environment["AGENTSPACE_SESSION_ID"], "test");
            assert!(!spec.environment.contains_key("KERNEL_SESSION_ID"));
            assert_eq!(spec.environment["KERNEL_FREE_PORT"], "8081");
            assert_eq!(
                spec.ports,
                vec![
                    PortBinding {
                        container_port: 8081,
                        host_ip: "0.0.0.0".to_owned()
                    },
                    PortBinding {
                        container_port: 8080,
                        host_ip: "0.0.0.0".to_owned()
                    },
                ]
            );
            assert!(spec.volumes.contains(&VolumeMount {
                volume_name: "agentspace-session-workspace-test".to_owned(),
                bind: "/workspace".to_owned(),
                mode: "rw".to_owned(),
            }));
            assert!(spec.volumes.contains(&VolumeMount {
                volume_name: "agentspace-session-telemetry-test".to_owned(),
                bind: "/var/lib/agentspace/telemetry".to_owned(),
                mode: "rw".to_owned(),
            }));
            assert!(spec.volumes.contains(&VolumeMount {
                volume_name: "agentspace-workspace-todo-list-code".to_owned(),
                bind: "/workspace/todo-list-code".to_owned(),
                mode: "rw".to_owned(),
            }));
            assert!(spec.volumes.contains(&VolumeMount {
                volume_name: "agentspace-workspace-todo-list-items".to_owned(),
                bind: "/workspace/todo-list-items".to_owned(),
                mode: "ro".to_owned(),
            }));
            assert_eq!(
                spec.entrypoint,
                vec![
                    "/usr/local/bin/uv",
                    "run",
                    "--no-dev",
                    "--package",
                    "kernel-host",
                    "-m",
                    "kernel_host.api_main",
                ]
            );
            drop(state);
        }
    }

    #[test]
    fn copilot_kernel_environment_uses_durable_session_workspace_paths() {
        let runtime = DockerKernelRuntime::new(
            DockerRuntimeConfig::default(),
            Arc::new(FakeDockerBackend::default()),
        );
        let mut env = BTreeMap::new();
        env.insert(
            "KERNEL_SESSION_ID".to_owned(),
            "8e0c8f12-d32f-4e40-b25c-5bb6ee4f6967".to_owned(),
        );
        let request = RuntimeCreateSession {
            session_id: "durable-session".to_owned(),
            telemetry_volume_identity: None,
            harness: HarnessName::CopilotCli,
            interaction_mode: InteractionMode::Chat,
            env,
            additional_paths: Vec::new(),
            skills: vec!["alpha".to_owned()],
            skill_volumes: Vec::new(),
            workspace_mounts: Vec::new(),
        };

        let environment = runtime.kernel_environment(&request);

        assert_eq!(
            environment["KERNEL_SESSION_ID"],
            "8e0c8f12-d32f-4e40-b25c-5bb6ee4f6967"
        );
        assert_eq!(environment["AGENTSPACE_SESSION_ID"], "durable-session");
        assert_eq!(
            environment["KERNEL_SKILLS_DIR"],
            "/workspace/.github/skills"
        );
        assert_eq!(environment["KERNEL_SKILLS_STAGING_DIR"], "/mnt/all-skills");
        assert_eq!(
            environment["KERNEL_LEGACY_COPILOT_SKILLS_DIR"],
            "/root/.copilot/skills"
        );
        assert_eq!(environment["KERNEL_ENABLED_SKILLS"], "alpha");
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn create_adopts_running_container_and_reuses_volume_on_recreate() {
        let backend = FakeDockerBackend::default();
        let (runtime, health_server) = runtime_with_health(&backend).await;
        let request = runtime_request_with_telemetry("1234567890ab-full-session");

        let first = runtime
            .create_session(request.clone())
            .await
            .unwrap_or_else(|error| panic!("initial create failed: {error}"));
        let second = runtime
            .create_session(request.clone())
            .await
            .unwrap_or_else(|error| panic!("adoption failed: {error}"));

        assert_eq!(first, second);
        {
            let state = backend.state();
            assert_eq!(state.run_specs.len(), 1);
            let spec = &state.run_specs[0];
            assert_eq!(
                spec.labels.get("agentspace.session_id").map(String::as_str),
                Some("1234567890ab-full-session")
            );
            assert_eq!(
                spec.labels.get("agentspace.role").map(String::as_str),
                Some("kernel")
            );
            assert_eq!(
                spec.labels.get("agentspace.managed").map(String::as_str),
                Some("true")
            );
            assert_eq!(
                spec.labels
                    .get("agentspace.interaction_mode")
                    .map(String::as_str),
                Some("chat")
            );
            assert!(spec.volumes.contains(&VolumeMount {
                volume_name: "agentspace-session-telemetry-1234567890ab".to_owned(),
                bind: "/var/lib/agentspace/telemetry".to_owned(),
                mode: "rw".to_owned(),
            }));
            let volume_labels = state
                .volume_labels
                .get("agentspace-session-workspace-1234567890ab")
                .unwrap_or_else(|| panic!("session workspace volume was not created"));
            assert_eq!(
                volume_labels
                    .get("agentspace.session_id")
                    .map(String::as_str),
                Some("1234567890ab-full-session")
            );
            assert_eq!(
                volume_labels.get("agentspace.role").map(String::as_str),
                Some("session-workspace")
            );
            assert_eq!(
                volume_labels.get("agentspace.managed").map(String::as_str),
                Some("true")
            );
            assert_eq!(
                volume_labels
                    .get("agentspace.interaction_mode")
                    .map(String::as_str),
                Some("chat")
            );
            let telemetry_labels = state
                .volume_labels
                .get("agentspace-session-telemetry-1234567890ab")
                .unwrap_or_else(|| panic!("session telemetry volume was not created"));
            assert_eq!(
                telemetry_labels
                    .get("agentspace.session_id")
                    .map(String::as_str),
                Some("1234567890ab-full-session")
            );
            assert_eq!(
                telemetry_labels.get("agentspace.role").map(String::as_str),
                Some("session-telemetry")
            );
            assert_eq!(
                telemetry_labels
                    .get("agentspace.managed")
                    .map(String::as_str),
                Some("true")
            );
            assert!(!telemetry_labels.contains_key("agentspace.interaction_mode"));
            assert!(!telemetry_labels.contains_key("agentspace.resource_id"));
            drop(state);
        }

        {
            let mut state = backend.state();
            let container = state
                .containers
                .get_mut("agentspace-kernel-1234567890ab")
                .unwrap_or_else(|| panic!("kernel container was not created"));
            container.running = false;
            "exited".clone_into(&mut container.status);
            state.running.remove("agentspace-kernel-1234567890ab");
        }
        runtime
            .create_session(request)
            .await
            .unwrap_or_else(|error| panic!("recreate failed: {error}"));

        let state = backend.state();
        assert_eq!(state.run_specs.len(), 2);
        assert_eq!(
            state
                .created_volumes
                .iter()
                .filter(|(name, _labels)| { name == "agentspace-session-workspace-1234567890ab" })
                .count(),
            1
        );
        assert_eq!(
            state
                .created_volumes
                .iter()
                .filter(|(name, _labels)| { name == "agentspace-session-telemetry-1234567890ab" })
                .count(),
            1
        );
        assert_eq!(
            state.removed_containers,
            vec!["agentspace-kernel-1234567890ab"]
        );
        drop(state);
        health_server.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn cli_recovery_recreates_missing_container_with_durable_state() {
        let backend = FakeDockerBackend::default();
        let (runtime, health_server) = runtime_with_health(&backend).await;
        let session_id = "12345678-1234-5678-9234-567812345678";
        let mut request = runtime_request(session_id);
        request.telemetry_volume_identity = Some(session_id.to_owned());
        request.harness = HarnessName::CopilotCli;
        request.interaction_mode = InteractionMode::Cli;

        runtime
            .create_session(request.clone())
            .await
            .unwrap_or_else(|error| panic!("initial CLI create failed: {error}"));
        {
            let mut state = backend.state();
            state.containers.clear();
            state.running.clear();
        }
        request
            .env
            .insert(RUNTIME_RECOVERY_ENV.to_owned(), "1".to_owned());

        runtime
            .create_session(request.clone())
            .await
            .unwrap_or_else(|error| panic!("CLI recovery failed: {error}"));

        {
            let state = backend.state();
            assert_eq!(state.run_specs.len(), 2);
            assert_eq!(
                state.run_specs[1]
                    .environment
                    .get(TERMINAL_RESUME_ENV)
                    .map(String::as_str),
                Some("1")
            );
            assert!(
                !state.run_specs[1]
                    .environment
                    .contains_key(RUNTIME_RECOVERY_ENV)
            );
            assert_eq!(
                state
                    .created_volumes
                    .iter()
                    .filter(|(name, _labels)| {
                        name == "agentspace-session-workspace-12345678-123"
                    })
                    .count(),
                1
            );
            assert_eq!(
                state
                    .created_volumes
                    .iter()
                    .filter(|(name, _labels)| {
                        name == "agentspace-session-telemetry-12345678-123"
                    })
                    .count(),
                1
            );
            assert!(state.volumes.contains("agentspace-kernel_copilot-config"));
            drop(state);
        }

        {
            let mut state = backend.state();
            state.containers.clear();
            state.running.clear();
            state
                .volumes
                .remove("agentspace-session-workspace-12345678-123");
            state
                .volume_labels
                .remove("agentspace-session-workspace-12345678-123");
            state
                .volumes
                .remove("agentspace-session-telemetry-12345678-123");
            state
                .volume_labels
                .remove("agentspace-session-telemetry-12345678-123");
        }
        let result = runtime.create_session(request.clone()).await;
        let Err(error) = result else {
            panic!("missing durable workspace must fail");
        };
        assert!(
            error
                .to_string()
                .contains("durable session workspace volume")
        );

        {
            let mut state = backend.state();
            seed_volume(
                &mut state,
                "agentspace-session-workspace-12345678-123",
                session_workspace_labels(&request),
            );
            seed_volume(
                &mut state,
                "agentspace-session-telemetry-12345678-123",
                session_telemetry_labels(&request),
            );
            state.volumes.remove("agentspace-kernel_copilot-config");
            state
                .volume_labels
                .remove("agentspace-kernel_copilot-config");
        }
        let result = runtime.create_session(request.clone()).await;
        let Err(error) = result else {
            panic!("missing durable Copilot state must fail");
        };
        assert!(error.to_string().contains("durable Copilot state volume"));

        {
            let mut state = backend.state();
            state
                .volumes
                .insert("agentspace-kernel_copilot-config".to_owned());
            state.volume_labels.insert(
                "agentspace-kernel_copilot-config".to_owned(),
                btree_map([
                    ("agentspace.role", "copilot-state"),
                    ("agentspace.managed", "true"),
                ]),
            );
            state
                .volumes
                .remove("agentspace-session-telemetry-12345678-123");
            state
                .volume_labels
                .remove("agentspace-session-telemetry-12345678-123");
        }
        let result = runtime.create_session(request).await;
        let Err(error) = result else {
            panic!("missing durable telemetry volume must fail");
        };
        assert!(
            error
                .to_string()
                .contains("durable session telemetry volume")
        );
        health_server.abort();
    }

    #[tokio::test]
    async fn adoption_uses_full_labels_instead_of_cosmetic_names() {
        let backend = FakeDockerBackend::default();
        let (runtime, health_server) = runtime_with_health(&backend).await;
        {
            let mut state = backend.state();
            seed_volume(
                &mut state,
                "renamed-workspace-volume",
                btree_map([
                    ("agentspace.role", "session-workspace"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "full-label-identity"),
                    ("agentspace.interaction_mode", "chat"),
                ]),
            );
            seed_volume(
                &mut state,
                "renamed-telemetry-volume",
                btree_map([
                    ("agentspace.role", "session-telemetry"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "full-label-identity"),
                ]),
            );
            seed_container_with_mounts(
                &mut state,
                "renamed-kernel-container",
                btree_map([
                    ("agentspace.role", "kernel"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "full-label-identity"),
                    ("agentspace.interaction_mode", "chat"),
                    ("agentspace.harness", "echo"),
                ]),
                true,
                "running",
                BTreeMap::from([
                    (
                        SESSION_WORKSPACE_MOUNT_PATH.to_owned(),
                        "renamed-workspace-volume".to_owned(),
                    ),
                    (
                        SESSION_TELEMETRY_MOUNT_PATH.to_owned(),
                        "renamed-telemetry-volume".to_owned(),
                    ),
                ]),
            );
        }

        let session = runtime
            .create_session(runtime_request_with_telemetry("full-label-identity"))
            .await
            .unwrap_or_else(|error| panic!("label-based adoption failed: {error}"));

        let KernelRuntimeSession::Docker(session) = session else {
            panic!("expected Docker session");
        };
        assert_eq!(session.container_name, "renamed-kernel-container");
        assert_eq!(
            session.session_workspace_volume_name,
            "renamed-workspace-volume"
        );
        assert_eq!(
            session.session_telemetry_volume_name.as_deref(),
            Some("renamed-telemetry-volume")
        );
        assert!(backend.state().run_specs.is_empty());
        health_server.abort();
    }

    #[tokio::test]
    async fn adoption_rejects_running_container_with_wrong_telemetry_mount() {
        let backend = FakeDockerBackend::default();
        let (runtime, health_server) = runtime_with_health(&backend).await;
        {
            let mut state = backend.state();
            seed_volume(
                &mut state,
                "expected-workspace",
                btree_map([
                    ("agentspace.role", "session-workspace"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "telemetry-mismatch"),
                    ("agentspace.interaction_mode", "chat"),
                ]),
            );
            seed_volume(
                &mut state,
                "expected-telemetry",
                btree_map([
                    ("agentspace.role", "session-telemetry"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "telemetry-mismatch"),
                ]),
            );
            seed_container_with_mounts(
                &mut state,
                "renamed-kernel-container",
                btree_map([
                    ("agentspace.role", "kernel"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "telemetry-mismatch"),
                    ("agentspace.interaction_mode", "chat"),
                    ("agentspace.harness", "echo"),
                ]),
                true,
                "running",
                BTreeMap::from([(
                    SESSION_WORKSPACE_MOUNT_PATH.to_owned(),
                    "expected-workspace".to_owned(),
                )]),
            );
        }

        let result = runtime
            .create_session(runtime_request_with_telemetry("telemetry-mismatch"))
            .await;
        let Err(error) = result else {
            panic!("missing telemetry mount must fail adoption");
        };
        assert!(error.to_string().contains("session telemetry mount"));
        health_server.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn create_rejects_name_and_label_collisions() {
        let backend = FakeDockerBackend::default();
        let (runtime, health_server) = runtime_with_health(&backend).await;
        {
            let mut state = backend.state();
            seed_container(
                &mut state,
                "agentspace-kernel-collision-se",
                btree_map([
                    ("agentspace.role", "kernel"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "different-session"),
                    ("agentspace.interaction_mode", "chat"),
                    ("agentspace.harness", "echo"),
                ]),
                true,
                "running",
                None,
            );
        }

        let result = runtime
            .create_session(runtime_request("collision-session"))
            .await;
        let Err(error) = result else {
            panic!("container name collision should fail");
        };
        assert!(matches!(error, AgentHostError::Conflict { .. }));

        let volume_backend = FakeDockerBackend::default();
        let (volume_runtime, volume_health_server) = runtime_with_health(&volume_backend).await;
        {
            let mut state = volume_backend.state();
            seed_volume(
                &mut state,
                "agentspace-session-workspace-volume-colli",
                btree_map([
                    ("agentspace.role", "session-workspace"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "different-session"),
                    ("agentspace.interaction_mode", "chat"),
                ]),
            );
        }
        let result = volume_runtime
            .create_session(runtime_request("volume-collision"))
            .await;
        let Err(error) = result else {
            panic!("volume name collision should fail");
        };
        assert!(matches!(error, AgentHostError::Conflict { .. }));

        let telemetry_backend = FakeDockerBackend::default();
        let (telemetry_runtime, telemetry_health_server) =
            runtime_with_health(&telemetry_backend).await;
        {
            let mut state = telemetry_backend.state();
            seed_volume(
                &mut state,
                "agentspace-session-telemetry-telemetry-co",
                btree_map([
                    ("agentspace.role", "session-telemetry"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "different-session"),
                ]),
            );
        }
        let result = telemetry_runtime
            .create_session(runtime_request_with_telemetry("telemetry-collision"))
            .await;
        let Err(error) = result else {
            panic!("telemetry volume name collision should fail");
        };
        assert!(matches!(error, AgentHostError::Conflict { .. }));

        let harness_backend = FakeDockerBackend::default();
        let (harness_runtime, harness_health_server) = runtime_with_health(&harness_backend).await;
        {
            let mut state = harness_backend.state();
            seed_volume(
                &mut state,
                "harness-workspace",
                btree_map([
                    ("agentspace.role", "session-workspace"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "harness-collision"),
                    ("agentspace.interaction_mode", "chat"),
                ]),
            );
            seed_container(
                &mut state,
                "harness-container",
                btree_map([
                    ("agentspace.role", "kernel"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "harness-collision"),
                    ("agentspace.interaction_mode", "chat"),
                    ("agentspace.harness", "copilot-cli"),
                ]),
                true,
                "running",
                Some("harness-workspace"),
            );
        }
        let result = harness_runtime
            .create_session(runtime_request("harness-collision"))
            .await;
        let Err(error) = result else {
            panic!("stale harness label should fail");
        };
        assert!(matches!(error, AgentHostError::Conflict { .. }));

        health_server.abort();
        volume_health_server.abort();
        telemetry_health_server.abort();
        harness_health_server.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn cleanup_reports_and_deletes_only_unowned_managed_resources() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        {
            let mut state = backend.state();
            for (name, session_id, running, status) in [
                ("owned-kernel", "owned", true, "running"),
                ("orphan-running", "orphan-running", true, "running"),
                ("orphan-exited", "orphan-exited", false, "exited"),
            ] {
                seed_container(
                    &mut state,
                    name,
                    btree_map([
                        ("agentspace.role", "kernel"),
                        ("agentspace.managed", "true"),
                        ("agentspace.session_id", session_id),
                        ("agentspace.interaction_mode", "chat"),
                    ]),
                    running,
                    status,
                    None,
                );
            }
            seed_container(
                &mut state,
                "legacy-kernel",
                btree_map([("agentspace.role", "kernel")]),
                true,
                "running",
                None,
            );
            for (name, session_id) in [
                ("owned-volume", Some("owned")),
                ("orphan-volume", Some("orphan-volume")),
                ("unclaimed-volume", None),
            ] {
                let mut labels = btree_map([
                    ("agentspace.role", "session-workspace"),
                    ("agentspace.managed", "true"),
                    ("agentspace.interaction_mode", "chat"),
                ]);
                if let Some(session_id) = session_id {
                    labels.insert("agentspace.session_id".to_owned(), session_id.to_owned());
                }
                seed_volume(&mut state, name, labels);
            }
            for (name, session_id) in [
                ("owned-telemetry", Some("owned")),
                ("orphan-telemetry", Some("orphan-telemetry")),
                ("unclaimed-telemetry", None),
            ] {
                let mut labels = btree_map([
                    ("agentspace.role", "session-telemetry"),
                    ("agentspace.managed", "true"),
                ]);
                if let Some(session_id) = session_id {
                    labels.insert("agentspace.session_id".to_owned(), session_id.to_owned());
                }
                seed_volume(&mut state, name, labels);
            }
        }
        let owned = BTreeSet::from(["owned".to_owned()]);

        let report = runtime
            .cleanup_orphans(&owned, true, None)
            .await
            .unwrap_or_else(|error| panic!("dry-run cleanup failed: {error}"));
        assert_eq!(report.resources.len(), 6);
        assert!(
            report
                .resources
                .iter()
                .all(|resource| resource.action == CleanupAction::WouldDelete)
        );
        assert!(report.resources.iter().any(|resource| {
            resource.name == "orphan-running" && resource.status.as_deref() == Some("running")
        }));
        assert!(report.resources.iter().any(|resource| {
            resource.name == "orphan-exited" && resource.status.as_deref() == Some("exited")
        }));
        assert!(backend.state().removed_containers.is_empty());

        let reviewed = report
            .resources
            .iter()
            .map(|resource| CleanupResourceIdentity {
                kind: resource.kind,
                name: resource.name.clone(),
                resource_id: resource.resource_id.clone(),
                session_id: resource.session_id.clone(),
            })
            .collect::<Vec<_>>();
        let report = runtime
            .cleanup_orphans(&owned, false, Some(&reviewed))
            .await
            .unwrap_or_else(|error| panic!("cleanup failed: {error}"));
        assert_eq!(report.deleted_count, 6);
        assert_eq!(report.error_count, 0);
        let state = backend.state();
        assert!(state.containers.contains_key("owned-kernel"));
        assert!(state.containers.contains_key("legacy-kernel"));
        assert!(state.volume_labels.contains_key("owned-volume"));
        assert!(state.volume_labels.contains_key("owned-telemetry"));
        assert!(!state.containers.contains_key("orphan-running"));
        assert!(!state.containers.contains_key("orphan-exited"));
        assert!(!state.volume_labels.contains_key("orphan-volume"));
        assert!(!state.volume_labels.contains_key("unclaimed-volume"));
        assert!(!state.volume_labels.contains_key("orphan-telemetry"));
        assert!(!state.volume_labels.contains_key("unclaimed-telemetry"));
        drop(state);
    }

    #[tokio::test]
    async fn cleanup_apply_refuses_resources_changed_after_review() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        {
            let mut state = backend.state();
            seed_container(
                &mut state,
                "orphan",
                btree_map([
                    ("agentspace.role", "kernel"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "orphan"),
                    ("agentspace.interaction_mode", "chat"),
                ]),
                true,
                "running",
                None,
            );
        }
        let report = runtime
            .cleanup_orphans(&BTreeSet::new(), true, None)
            .await
            .unwrap_or_else(|error| panic!("review failed: {error}"));
        let reviewed = report
            .resources
            .iter()
            .map(|resource| CleanupResourceIdentity {
                kind: resource.kind,
                name: resource.name.clone(),
                resource_id: resource.resource_id.clone(),
                session_id: resource.session_id.clone(),
            })
            .collect::<Vec<_>>();
        backend
            .state()
            .containers
            .get_mut("orphan")
            .unwrap_or_else(|| panic!("missing seeded orphan"))
            .resource_id = "replacement-container".to_owned();

        let applied = runtime
            .cleanup_orphans(&BTreeSet::new(), false, Some(&reviewed))
            .await
            .unwrap_or_else(|error| panic!("apply failed: {error}"));

        assert_eq!(applied.deleted_count, 0);
        assert_eq!(applied.error_count, 1);
        assert!(backend.state().containers.contains_key("orphan"));
    }

    #[tokio::test]
    async fn cleanup_apply_refuses_resource_that_became_durably_owned() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        {
            let mut state = backend.state();
            seed_container(
                &mut state,
                "orphan",
                btree_map([
                    ("agentspace.role", "kernel"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "adopted"),
                    ("agentspace.interaction_mode", "chat"),
                ]),
                true,
                "running",
                None,
            );
        }
        let report = runtime
            .cleanup_orphans(&BTreeSet::new(), true, None)
            .await
            .unwrap_or_else(|error| panic!("review failed: {error}"));
        let reviewed = report
            .resources
            .iter()
            .map(|resource| CleanupResourceIdentity {
                kind: resource.kind,
                name: resource.name.clone(),
                resource_id: resource.resource_id.clone(),
                session_id: resource.session_id.clone(),
            })
            .collect::<Vec<_>>();

        let applied = runtime
            .cleanup_orphans(
                &BTreeSet::from(["adopted".to_owned()]),
                false,
                Some(&reviewed),
            )
            .await
            .unwrap_or_else(|error| panic!("apply failed: {error}"));

        assert_eq!(applied.deleted_count, 0);
        assert_eq!(applied.error_count, 1);
        assert!(backend.state().containers.contains_key("orphan"));
    }

    #[tokio::test]
    async fn delete_by_stable_identity_removes_container_workspace_and_telemetry() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        {
            let mut state = backend.state();
            let labels = btree_map([
                ("agentspace.role", "kernel"),
                ("agentspace.managed", "true"),
                ("agentspace.session_id", "stable-delete"),
                ("agentspace.interaction_mode", "chat"),
            ]);
            seed_container(
                &mut state,
                "cosmetic-container-name",
                labels,
                true,
                "running",
                Some("cosmetic-volume-name"),
            );
            seed_volume(
                &mut state,
                "cosmetic-volume-name",
                btree_map([
                    ("agentspace.role", "session-workspace"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "stable-delete"),
                    ("agentspace.interaction_mode", "chat"),
                ]),
            );
            seed_volume(
                &mut state,
                "cosmetic-telemetry-volume",
                btree_map([
                    ("agentspace.role", "session-telemetry"),
                    ("agentspace.managed", "true"),
                    ("agentspace.session_id", "stable-delete"),
                ]),
            );
        }

        runtime
            .destroy_session_by_id("stable-delete")
            .await
            .unwrap_or_else(|error| panic!("delete by identity failed: {error}"));

        let state = backend.state();
        assert_eq!(state.removed_containers, vec!["cosmetic-container-name"]);
        assert_eq!(
            state.removed_volumes,
            vec!["cosmetic-volume-name", "cosmetic-telemetry-volume"]
        );
        drop(state);
    }

    #[tokio::test]
    async fn skill_volume_is_shared_and_retained_when_sessions_are_destroyed() {
        let backend = FakeDockerBackend::default();
        let mut config = DockerRuntimeConfig::default();
        config.skill_volume_overrides.insert(
            "memory/data".to_owned(),
            "agentspace-memory-data".to_owned(),
        );
        let runtime = DockerKernelRuntime::new(config, Arc::new(backend.clone()));
        let request = RuntimeCreateSession {
            session_id: "memory-enabled".to_owned(),
            telemetry_volume_identity: None,
            harness: HarnessName::Echo,
            interaction_mode: InteractionMode::Chat,
            env: BTreeMap::new(),
            additional_paths: Vec::new(),
            skills: vec!["memory".to_owned()],
            skill_volumes: vec![SkillVolumeResource {
                skill_id: "memory".to_owned(),
                resource_id: "data".to_owned(),
                mount_path: "/var/lib/agentspace/memory".to_owned(),
                advertise: false,
                mode: SkillVolumeMode::Rw,
            }],
            workspace_mounts: Vec::new(),
        };

        runtime
            .run_kernel_container(
                "agentspace-kernel-one",
                "agentspace-session-workspace-one",
                None,
                &request,
            )
            .await
            .unwrap_or_else(|error| panic!("first run failed: {error}"));
        runtime
            .run_kernel_container(
                "agentspace-kernel-two",
                "agentspace-session-workspace-two",
                None,
                &request,
            )
            .await
            .unwrap_or_else(|error| panic!("second run failed: {error}"));
        runtime
            .destroy_session(KernelRuntimeSession::Docker(DockerKernelSession {
                session_id: "memory-enabled".to_owned(),
                container_name: "agentspace-kernel-one".to_owned(),
                session_workspace_volume_name: "agentspace-session-workspace-one".to_owned(),
                session_telemetry_volume_name: None,
                base_url: "http://agentspace-kernel-one:8000".to_owned(),
                vscode_url: None,
                free_port_url: None,
            }))
            .await
            .unwrap_or_else(|error| panic!("destroy failed: {error}"));

        let state = backend.state();
        assert_eq!(
            state
                .created_volumes
                .iter()
                .filter(|(name, _labels)| name == "agentspace-memory-data")
                .count(),
            1
        );
        let labels = state
            .created_volumes
            .iter()
            .find_map(|(name, labels)| (name == "agentspace-memory-data").then_some(labels))
            .unwrap_or_else(|| panic!("memory volume was not created"));
        assert_eq!(
            labels.get("agentspace.role").map(String::as_str),
            Some("skill-resource")
        );
        assert_eq!(
            labels.get("agentspace.skill_id").map(String::as_str),
            Some("memory")
        );
        assert!(state.run_specs.iter().all(|spec| {
            spec.volumes.contains(&VolumeMount {
                volume_name: "agentspace-memory-data".to_owned(),
                bind: "/var/lib/agentspace/memory".to_owned(),
                mode: "rw".to_owned(),
            })
        }));
        assert_eq!(
            state.removed_volumes,
            vec!["agentspace-session-workspace-one"]
        );
        drop(state);
    }

    #[test]
    fn container_create_body_matches_docker_run_shape() {
        let spec = ContainerRunSpec {
            image: "image:latest".to_owned(),
            auto_remove: true,
            detach: true,
            entrypoint: vec!["/bin/echo".to_owned(), "hello".to_owned()],
            environment: btree_map([("KEY", "value")]),
            labels: btree_map([("agentspace.role", "test")]),
            name: Some("container-name".to_owned()),
            network: Some("agentspace".to_owned()),
            network_disabled: false,
            ports: vec![PortBinding {
                container_port: 8080,
                host_ip: "127.0.0.1".to_owned(),
            }],
            volumes: vec![VolumeMount {
                volume_name: "volume-name".to_owned(),
                bind: "/workspace".to_owned(),
                mode: "ro".to_owned(),
            }],
        };

        let body = container_create_body(&spec);
        assert_eq!(body.image, Some("image:latest".to_owned()));
        assert_eq!(body.entrypoint, Some(vec!["/bin/echo".to_owned()]));
        assert_eq!(body.cmd, Some(vec!["hello".to_owned()]));
        assert_eq!(body.env, Some(vec!["KEY=value".to_owned()]));
        assert_eq!(
            body.labels
                .and_then(|labels| labels.get("agentspace.role").cloned()),
            Some("test".to_owned())
        );
        let Some(host_config) = body.host_config else {
            panic!("host config should be set");
        };
        assert_eq!(host_config.auto_remove, Some(true));
        assert_eq!(host_config.network_mode, Some("agentspace".to_owned()));
        assert_eq!(
            host_config.binds,
            Some(vec!["volume-name:/workspace:ro".to_owned()])
        );
        let binding = host_config
            .port_bindings
            .and_then(|bindings| bindings.get("8080/tcp").cloned())
            .flatten()
            .and_then(|bindings| bindings.into_iter().next());
        assert_eq!(
            binding.as_ref().and_then(|binding| binding.host_ip.clone()),
            Some("127.0.0.1".to_owned())
        );
        assert_eq!(
            binding.and_then(|binding| binding.host_port),
            Some(String::new())
        );

        let mut foreground_spec = spec;
        foreground_spec.detach = false;
        let foreground_body = container_create_body(&foreground_spec);
        assert_eq!(
            foreground_body
                .host_config
                .and_then(|config| config.auto_remove),
            Some(false)
        );
    }

    #[tokio::test]
    async fn docker_runtime_keeps_free_port_when_vscode_disabled() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        let mut env = BTreeMap::new();
        env.insert("KERNEL_VSCODE_ENABLED".to_owned(), "0".to_owned());
        let request = RuntimeCreateSession {
            session_id: "test".to_owned(),
            telemetry_volume_identity: None,
            harness: HarnessName::Echo,
            interaction_mode: InteractionMode::Chat,
            env,
            additional_paths: Vec::new(),
            skills: Vec::new(),
            skill_volumes: Vec::new(),
            workspace_mounts: Vec::new(),
        };

        runtime
            .run_kernel_container(
                "agentspace-kernel-test",
                "agentspace-session-workspace-test",
                None,
                &request,
            )
            .await
            .unwrap_or_else(|error| panic!("run failed: {error}"));

        assert_eq!(
            backend.state().run_specs[0].ports,
            vec![PortBinding {
                container_port: 8081,
                host_ip: "0.0.0.0".to_owned(),
            }]
        );
    }

    #[test]
    fn vscode_proxy_uses_configured_agent_host_route() {
        let config = DockerRuntimeConfig {
            vscode_container_port: 9443,
            vscode_proxy_url_template:
                "https://proxy.internal/{container_name}/ports/{container_port}".to_owned(),
            ..DockerRuntimeConfig::default()
        };
        let runtime = DockerKernelRuntime::new(config, Arc::new(FakeDockerBackend::default()));
        let session = KernelRuntimeSession::Docker(DockerKernelSession {
            session_id: "session".to_owned(),
            container_name: "agentspace-kernel-session".to_owned(),
            session_workspace_volume_name: "workspace".to_owned(),
            session_telemetry_volume_name: None,
            base_url: "http://kernel".to_owned(),
            vscode_url: None,
            free_port_url: None,
        });

        assert_eq!(
            runtime
                .vscode_proxy_base_url(&session)
                .unwrap_or_else(|error| panic!("proxy URL should resolve: {error}")),
            "https://proxy.internal/agentspace-kernel-session/ports/9443"
        );
    }

    #[tokio::test]
    async fn docker_runtime_clones_workspace_volume() {
        let backend = FakeDockerBackend::default();
        backend
            .state()
            .volumes
            .insert("agentspace-workspace-source".to_owned());
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));

        let result = runtime
            .clone_workspace(
                "agentspace-workspace-source".to_owned(),
                "target".to_owned(),
                "agentspace-workspace-target".to_owned(),
            )
            .await
            .unwrap_or_else(|error| panic!("clone failed: {error}"));

        assert_eq!(
            result,
            json!({
                "workspace_id": "target",
                "volume_name": "agentspace-workspace-target"
            })
        );
        {
            let state = backend.state();
            let spec = &state.run_specs[0];
            assert!(!spec.detach);
            assert!(spec.network_disabled);
            assert_eq!(spec.labels["agentspace.role"], "workspace-snapshot");
            assert_eq!(spec.environment["AGENTSPACE_WORKSPACE_ID"], "target");
            assert_eq!(
                spec.volumes,
                vec![
                    VolumeMount {
                        volume_name: "agentspace-workspace-source".to_owned(),
                        bind: "/workspace-src".to_owned(),
                        mode: "ro".to_owned(),
                    },
                    VolumeMount {
                        volume_name: "agentspace-workspace-target".to_owned(),
                        bind: "/workspace-dest".to_owned(),
                        mode: "rw".to_owned(),
                    }
                ]
            );
            drop(state);
        }
    }

    #[tokio::test]
    async fn docker_runtime_snapshots_with_nested_relative_exclusions() {
        let backend = FakeDockerBackend::default();
        backend
            .state()
            .volumes
            .insert("agentspace-session-workspace-session".to_owned());
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        let session = KernelRuntimeSession::Docker(DockerKernelSession {
            session_id: "session".to_owned(),
            container_name: "agentspace-kernel-session".to_owned(),
            session_workspace_volume_name: "agentspace-session-workspace-session".to_owned(),
            session_telemetry_volume_name: None,
            base_url: "http://kernel".to_owned(),
            vscode_url: None,
            free_port_url: None,
        });
        let exclude_paths = vec![
            ".github/agents/agentspace-session.agent.md".to_owned(),
            ".github/skills/alpha".to_owned(),
        ];

        runtime
            .snapshot_session_workspace(
                &session,
                "target".to_owned(),
                "agentspace-workspace-target".to_owned(),
                exclude_paths.clone(),
            )
            .await
            .unwrap_or_else(|error| panic!("snapshot failed: {error}"));

        let state = backend.state();
        let exclude_paths_json =
            state.run_specs[0].environment["AGENTSPACE_WORKSPACE_EXCLUDE_PATHS_JSON"].clone();
        let snapshot_script = state.run_specs[0].entrypoint[2].clone();
        drop(state);
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&exclude_paths_json)
                .unwrap_or_else(|error| panic!("exclude paths were not JSON: {error}")),
            exclude_paths
        );
        assert!(snapshot_script.contains("agentspace-owned-profile"));
        assert!(snapshot_script.contains("PurePosixPath"));
    }

    #[tokio::test]
    async fn workspace_snapshot_rejects_identical_source_and_destination_volumes() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        let session = KernelRuntimeSession::Docker(DockerKernelSession {
            session_id: "session".to_owned(),
            container_name: "agentspace-kernel-session".to_owned(),
            session_workspace_volume_name: "agentspace-session-workspace-session".to_owned(),
            session_telemetry_volume_name: None,
            base_url: "http://kernel".to_owned(),
            vscode_url: None,
            free_port_url: None,
        });
        let source = match &session {
            KernelRuntimeSession::Docker(handle) => handle.session_workspace_volume_name.clone(),
            KernelRuntimeSession::Opaque(_) => panic!("expected Docker session"),
        };

        let Err(error) = runtime
            .snapshot_session_workspace(&session, "same".to_owned(), source, Vec::new())
            .await
        else {
            panic!("identical snapshot volumes must be rejected");
        };

        assert!(matches!(error, AgentHostError::Validation { .. }));
        assert!(backend.state().run_specs.is_empty());
    }

    #[tokio::test]
    async fn workspace_snapshot_propagates_helper_container_failure() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        let session = KernelRuntimeSession::Docker(DockerKernelSession {
            session_id: "session".to_owned(),
            container_name: "agentspace-kernel-session".to_owned(),
            session_workspace_volume_name: "agentspace-session-workspace-session".to_owned(),
            session_telemetry_volume_name: None,
            base_url: "http://kernel".to_owned(),
            vscode_url: None,
            free_port_url: None,
        });
        let source = match &session {
            KernelRuntimeSession::Docker(handle) => handle.session_workspace_volume_name.clone(),
            KernelRuntimeSession::Opaque(_) => panic!("expected Docker session"),
        };
        {
            let mut state = backend.state();
            state.volumes.insert(source);
            state.fail_next_run = true;
        }

        let Err(error) = runtime
            .snapshot_session_workspace(
                &session,
                "target".to_owned(),
                "agentspace-workspace-target".to_owned(),
                Vec::new(),
            )
            .await
        else {
            panic!("helper failure must fail the snapshot");
        };

        assert!(error.to_string().contains("status 1"));
    }

    #[tokio::test]
    async fn docker_runtime_opens_workspace_vscode() {
        let backend = FakeDockerBackend::default();
        backend
            .state()
            .volumes
            .insert("agentspace-workspace-todo-list-code".to_owned());
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));

        let result = runtime
            .open_workspace_vscode(
                "todo-list-code".to_owned(),
                "agentspace-workspace-todo-list-code".to_owned(),
            )
            .await
            .unwrap_or_else(|error| panic!("open failed: {error}"));

        assert_eq!(
            result,
            json!({
                "workspace_id": "todo-list-code",
                "volume_name": "agentspace-workspace-todo-list-code",
                "container_name": "agentspace-workspace-editor-todo-list-code",
                "vscode_url": "http://127.0.0.1:45678"
            })
        );
        {
            let state = backend.state();
            let spec = &state.run_specs[0];
            assert_eq!(
                spec.name.as_deref(),
                Some("agentspace-workspace-editor-todo-list-code")
            );
            assert_eq!(
                spec.entrypoint,
                vec![
                    "/usr/local/bin/code-server",
                    "--bind-addr",
                    "0.0.0.0:8080",
                    "--auth",
                    "none",
                    "--disable-telemetry",
                    "/workspace",
                ]
            );
            assert_eq!(
                spec.ports,
                vec![PortBinding {
                    container_port: 8080,
                    host_ip: "0.0.0.0".to_owned(),
                }]
            );
            drop(state);
        }
    }

    #[test]
    fn summarize_docker_stats_computes_percentages() {
        let raw = json!({
            "cpu_stats": {
                "cpu_usage": { "total_usage": 200 },
                "system_cpu_usage": 1000,
                "online_cpus": 2
            },
            "precpu_stats": {
                "cpu_usage": { "total_usage": 100 },
                "system_cpu_usage": 500
            },
            "memory_stats": {
                "usage": 200,
                "limit": 1000,
                "stats": { "cache": 50 }
            }
        });

        let summary = summarize_docker_stats(&docker_stats(raw))
            .unwrap_or_else(|| panic!("stats summary should be present"));

        assert_eq!(summary.cpu_percent, Some(40.0));
        assert_eq!(summary.memory_usage_bytes, Some(150));
        assert_eq!(summary.memory_limit_bytes, Some(1000));
        assert_eq!(summary.memory_percent, Some(15.0));
    }

    #[test]
    fn summarize_docker_stats_handles_missing_fields() {
        assert!(summarize_docker_stats(&docker_stats(json!({}))).is_none());
    }

    #[test]
    fn summarize_docker_stats_uses_cgroup_v2_inactive_file() {
        let summary = summarize_docker_stats(&docker_stats(json!({
            "memory_stats": {
                "usage": 200,
                "limit": 1000,
                "stats": { "inactive_file": 75 }
            }
        })))
        .unwrap_or_else(|| panic!("stats summary should be present"));

        assert_eq!(summary.memory_usage_bytes, Some(125));
        assert!(summary.cpu_percent.is_none());
    }

    #[test]
    fn skills_mount_paths_cover_harnesses() {
        assert_eq!(
            skills_mount_path(HarnessName::Acp),
            "/workspace/.agents/skills"
        );
        assert_eq!(
            skills_mount_path(HarnessName::CopilotCli),
            "/workspace/.github/skills"
        );
        assert_eq!(
            skills_mount_path(HarnessName::Opencode),
            "/root/.config/opencode/skills"
        );
        assert_eq!(skills_mount_path(HarnessName::Echo), "/skills");
        assert_eq!(skills_mount_path(HarnessName::ClaudeCode), "/skills");
        assert_eq!(skills_mount_path(HarnessName::Codex), "/skills");
    }

    #[test]
    fn session_workspace_volume_name_uses_container_suffix() {
        let volume = session_workspace_volume_name_from_container("agentspace-kernel-test")
            .unwrap_or_else(|error| panic!("volume failed: {error}"));

        assert_eq!(volume, "agentspace-session-workspace-test");
    }

    #[test]
    fn session_telemetry_volume_name_uses_identity_suffix() {
        assert_eq!(
            session_telemetry_volume_name("1234567890ab-full-session"),
            "agentspace-session-telemetry-1234567890ab"
        );
    }

    fn docker_stats(payload: serde_json::Value) -> DockerStats {
        serde_json::from_value(payload)
            .unwrap_or_else(|error| panic!("failed to parse docker stats fixture: {error}"))
    }
}
