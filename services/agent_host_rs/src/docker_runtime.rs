use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::Duration,
};

use async_stream::try_stream;
use async_trait::async_trait;
use bollard::{
    Docker,
    container::LogOutput,
    errors::Error as BollardError,
    models::{
        ContainerCreateBody, HostConfig, PortBinding as DockerPortBinding, PortMap,
        VolumeCreateRequest,
    },
    query_parameters::{
        CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
        RemoveVolumeOptionsBuilder, StatsOptionsBuilder, WaitContainerOptionsBuilder,
    },
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{sync::Mutex as AsyncMutex, time};

use crate::{
    errors::AgentHostError,
    models::{
        DockerKernelSession, DockerStatsSummary, HarnessName, KernelEvent, KernelRuntimeSession,
        RuntimeSessionSummary, ServiceSummary, WorkspaceMount,
    },
    sessions::{EventStream, KernelRuntime, RuntimeCreateSession},
};

const SESSION_WORKSPACE_MOUNT_PATH: &str = "/workspace";
const CONTAINER_NAME_PLACEHOLDER: &str = concat!("{", "container_name", "}");
const HOST_IP_PLACEHOLDER: &str = concat!("{", "host_ip", "}");
const HOST_PORT_PLACEHOLDER: &str = concat!("{", "host_port", "}");
const CONTAINER_PORT_PLACEHOLDER: &str = concat!("{", "container_port", "}");
const WORKSPACE_SNAPSHOT_SCRIPT: &str = r#"
from __future__ import annotations

import os
import pathlib
import shutil

source = pathlib.Path("/workspace-src")
dest = pathlib.Path("/workspace-dest")
exclude_env = os.environ.get("AGENTSPACE_WORKSPACE_EXCLUDES", "")
exclude = {item for item in exclude_env.split(",") if item}
dest.mkdir(parents=True, exist_ok=True)
for entry in source.iterdir():
    if entry.name in exclude:
        continue
    target = dest / entry.name
    if entry.is_symlink():
        target.symlink_to(os.readlink(entry))
    elif entry.is_dir():
        shutil.copytree(entry, target, symlinks=True, dirs_exist_ok=True)
    else:
        shutil.copy2(entry, target)
"#;

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
        request: &RuntimeCreateSession,
    ) -> Result<(), AgentHostError> {
        let environment = self.kernel_environment(request);
        let ports = self.kernel_ports(&environment);
        let volumes = self
            .kernel_volumes(container_name, &request.workspace_mounts)
            .await?;

        self.backend
            .run_container(ContainerRunSpec {
                image: self.config.kernel_image.clone(),
                auto_remove: true,
                detach: true,
                entrypoint: kernel_entrypoint(),
                environment,
                labels: btree_map([("agentspace.role", "kernel")]),
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
        environment.insert("KERNEL_HARNESS".to_owned(), request.harness.to_string());
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
        environment.insert("KERNEL_ENABLED_SKILLS".to_owned(), request.skills.join(","));
        environment
            .entry("KERNEL_VSCODE_ENABLED".to_owned())
            .or_insert_with(|| "1".to_owned());
        environment.insert(
            "KERNEL_FREE_PORT".to_owned(),
            self.config.free_port_container_port.to_string(),
        );
        environment.extend(self.config.gitagent_env.clone());
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
        container_name: &str,
        workspace_mounts: &[WorkspaceMount],
    ) -> Result<Vec<VolumeMount>, AgentHostError> {
        let session_workspace_volume =
            session_workspace_volume_name_from_container(container_name)?;
        self.backend
            .ensure_volume(
                &session_workspace_volume,
                btree_map([
                    ("agentspace.role", "session-workspace"),
                    ("agentspace.managed", "true"),
                    ("agentspace.container_name", container_name),
                ]),
            )
            .await?;

        let mut volumes = vec![
            VolumeMount {
                volume_name: session_workspace_volume,
                bind: SESSION_WORKSPACE_MOUNT_PATH.to_owned(),
                mode: "rw".to_owned(),
            },
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
        ];
        for mount in workspace_mounts {
            volumes.push(self.workspace_volume_mount(mount).await?);
        }
        Ok(volumes)
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
        exclude_names: &[String],
    ) -> Result<(), AgentHostError> {
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
                    ("AGENTSPACE_WORKSPACE_EXCLUDES", &exclude_names.join(",")),
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
}

#[async_trait]
impl KernelRuntime for DockerKernelRuntime {
    async fn create_session(
        &self,
        request: RuntimeCreateSession,
    ) -> Result<KernelRuntimeSession, AgentHostError> {
        let container_name = kernel_container_name(&request.session_id);
        let base_url = self
            .config
            .base_url_template
            .replace(CONTAINER_NAME_PLACEHOLDER, &container_name);
        self.run_kernel_container(&container_name, &request).await?;
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
            container_name,
            session_workspace_volume_name: session_workspace_volume_name(&request.session_id),
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

    fn free_port_url(&self, session: &KernelRuntimeSession) -> Option<String> {
        Self::docker_session(session)
            .ok()
            .and_then(|handle| handle.free_port_url.clone())
    }

    async fn destroy_session(&self, session: KernelRuntimeSession) -> Result<(), AgentHostError> {
        let handle = Self::docker_session(&session)?;
        self.backend
            .remove_container(&handle.container_name)
            .await?;
        self.backend
            .remove_volume(&handle.session_workspace_volume_name)
            .await
    }

    async fn snapshot_session_workspace(
        &self,
        session: &KernelRuntimeSession,
        workspace_id: String,
        volume_name: String,
        exclude_names: Vec<String>,
    ) -> Result<Value, AgentHostError> {
        let handle = Self::docker_session(session)?;
        self.copy_workspace_volume(
            &handle.session_workspace_volume_name,
            &workspace_id,
            &volume_name,
            &exclude_names,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerRuntimeConfig {
    pub kernel_image: String,
    pub kernel_network: String,
    pub base_url_template: String,
    pub vscode_container_port: u16,
    pub vscode_host_ip: String,
    pub vscode_url_template: String,
    pub free_port_container_port: u16,
    pub free_port_host_ip: String,
    pub free_port_url_template: String,
    pub startup_timeout: Duration,
    pub copilot_volume: String,
    pub skills_volume: String,
    pub skills_dir: String,
    pub gitagent_env: BTreeMap<String, String>,
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
            free_port_container_port: 8081,
            free_port_host_ip: "0.0.0.0".to_owned(),
            free_port_url_template: "http://127.0.0.1:{host_port}".to_owned(),
            startup_timeout: Duration::from_mins(1),
            copilot_volume: "agentspace-kernel_copilot-config".to_owned(),
            skills_volume: "agentspace-skills".to_owned(),
            skills_dir: "/skills".to_owned(),
            gitagent_env: BTreeMap::new(),
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
        config.gitagent_env = gitagent_env_from_process();
        config
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
                || Ok(()),
                |result| result.map(|_| ()).map_err(AgentHostError::from),
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
        let docker = self.docker()?;
        match docker
            .inspect_container(
                container_name,
                None::<bollard::query_parameters::InspectContainerOptions>,
            )
            .await
        {
            Ok(inspect) => Ok(inspect
                .state
                .and_then(|state| state.running)
                .unwrap_or(false)),
            Err(error) if is_bollard_not_found(&error) => Ok(false),
            Err(error) => Err(error.into()),
        }
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

fn hash_map_from_btree(map: &BTreeMap<String, String>) -> HashMap<String, String> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn append_log_output(buffer: &mut Vec<u8>, output: &LogOutput) {
    buffer.extend_from_slice(output.as_ref());
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

fn gitagent_env_from_process() -> BTreeMap<String, String> {
    [
        ("AGENT_HOST_GITAGENT_REMOTE_URL", "GITAGENT_REMOTE_URL"),
        ("AGENT_HOST_GITAGENT_PATCH_URL", "GITAGENT_PATCH_URL"),
        (
            "AGENT_HOST_GITAGENT_DEFAULT_BRANCH",
            "GITAGENT_DEFAULT_BRANCH",
        ),
    ]
    .into_iter()
    .filter_map(|(host_name, container_name)| {
        std::env::var(host_name)
            .ok()
            .map(|value| (container_name.to_owned(), value))
    })
    .collect()
}

const fn path_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

const fn skills_mount_path(harness: HarnessName) -> &'static str {
    match harness {
        HarnessName::Acp => "/workspace/.agents/skills",
        HarnessName::ClaudeCode | HarnessName::Codex | HarnessName::Echo => "/skills",
        HarnessName::CopilotCli => "/root/.copilot/skills",
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
    };

    use async_trait::async_trait;
    use serde_json::json;

    use super::{
        ContainerRunSpec, DockerBackend, DockerKernelRuntime, DockerRuntimeConfig, DockerStats,
        PortBinding, VolumeMount, btree_map, container_create_body,
        session_workspace_volume_name_from_container, summarize_docker_stats,
    };
    use crate::{
        docker_runtime::skills_mount_path,
        errors::AgentHostError,
        models::{HarnessName, WorkspaceMount, WorkspaceMountMode},
        sessions::{KernelRuntime, RuntimeCreateSession},
    };

    #[derive(Clone, Default)]
    struct FakeDockerBackend {
        state: Arc<Mutex<FakeDockerState>>,
    }

    #[derive(Default)]
    struct FakeDockerState {
        volumes: BTreeSet<String>,
        created_volumes: Vec<(String, BTreeMap<String, String>)>,
        run_specs: Vec<ContainerRunSpec>,
        removed_containers: Vec<String>,
        removed_volumes: Vec<String>,
        running: BTreeSet<String>,
        ports: BTreeMap<(String, u16), u16>,
    }

    impl FakeDockerBackend {
        fn state(&self) -> MutexGuard<'_, FakeDockerState> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }
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
                if let Some(name) = &spec.name {
                    state.running.insert(name.clone());
                    state.ports.insert((name.clone(), 8080), 45_678);
                    state.ports.insert((name.clone(), 8081), 45_679);
                }
                state.run_specs.push(spec);
            }
            Ok(())
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
            self.state()
                .removed_containers
                .push(container_name.to_owned());
            Ok(())
        }

        async fn remove_volume(&self, volume_name: &str) -> Result<(), AgentHostError> {
            self.state().removed_volumes.push(volume_name.to_owned());
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
    }

    #[tokio::test]
    async fn docker_runtime_builds_kernel_container_spec() {
        let backend = FakeDockerBackend::default();
        let runtime =
            DockerKernelRuntime::new(DockerRuntimeConfig::default(), Arc::new(backend.clone()));
        let request = RuntimeCreateSession {
            session_id: "test".to_owned(),
            harness: HarnessName::Echo,
            env: BTreeMap::new(),
            additional_paths: Vec::new(),
            skills: Vec::new(),
            workspace_mounts: vec![
                WorkspaceMount::new("todo-list-code", WorkspaceMountMode::ReadWrite),
                WorkspaceMount::new("todo-list-items", WorkspaceMountMode::ReadOnly),
            ],
        };

        runtime
            .run_kernel_container("agentspace-kernel-test", &request)
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
                    }
                ]
            );
            assert!(spec.volumes.contains(&VolumeMount {
                volume_name: "agentspace-session-workspace-test".to_owned(),
                bind: "/workspace".to_owned(),
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
            harness: HarnessName::Echo,
            env,
            additional_paths: Vec::new(),
            skills: Vec::new(),
            workspace_mounts: Vec::new(),
        };

        runtime
            .run_kernel_container("agentspace-kernel-test", &request)
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
            "/root/.copilot/skills"
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

    fn docker_stats(payload: serde_json::Value) -> DockerStats {
        serde_json::from_value(payload)
            .unwrap_or_else(|error| panic!("failed to parse docker stats fixture: {error}"))
    }
}
