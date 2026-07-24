use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    fs,
    io::{self, Write},
    path::{Path, PathBuf, StripPrefixError},
    sync::{Arc, RwLock},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{AppState, models::ServiceSummary};

const ENV_SKILLS_DIR: &str = "AGENT_HOST_SKILLS_DIR";
const ENV_BUILTIN_SKILLS_DIR: &str = "AGENT_HOST_BUILTIN_SKILLS_DIR";
const DEFAULT_SKILLS_DIR: &str = "/skills";
const DEFAULT_BUILTIN_SKILLS_DIR: &str = "/builtin-skills";
const SKILL_VERSIONS_DIR: &str = ".skill-versions";
const SKILL_MARKDOWN_FILE: &str = "SKILL.md";
const SKILL_METADATA_FILE: &str = "agentspace.json";
const MARKDOWN_CONTENT_TYPE: &str = "text/markdown; charset=utf-8";
const ZIP_CONTENT_TYPE: &str = "application/zip";

#[derive(Clone)]
pub struct SkillRegistry {
    service: Arc<RwLock<SkillsService>>,
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new(SkillsService::from_env())
    }
}

impl SkillRegistry {
    #[must_use]
    pub fn new(service: SkillsService) -> Self {
        Self {
            service: Arc::new(RwLock::new(service)),
        }
    }

    pub fn from_synced_service(mut service: SkillsService) -> Result<Self, SkillError> {
        service.sync_builtin_skills()?;
        Ok(Self::new(service))
    }

    pub fn try_from_env() -> Result<Self, SkillError> {
        Self::from_synced_service(SkillsService::from_env())
    }

    #[must_use]
    pub const fn summary(&self) -> ServiceSummary {
        ServiceSummary::ready("filesystem-backed skill routes are active")
    }

    pub async fn create_skill(&self, request: CreateSkillRequest) -> Result<Skill, SkillError> {
        let service = self.service.clone();
        run_skill_task("create skill", move || {
            let service = service.write().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for create",
            })?;
            service.create_skill(&request.skill_id, &request.files)
        })
        .await
    }

    pub async fn list_skills(&self) -> Result<Vec<SkillSummary>, SkillError> {
        let service = self.service.clone();
        run_skill_task("list skills", move || {
            let service = service.read().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for list",
            })?;
            service.list_skills()
        })
        .await
    }

    pub async fn get_skill(&self, skill_id: &str) -> Result<Skill, SkillError> {
        let service = self.service.clone();
        let skill_id = skill_id.to_owned();
        run_skill_task("get skill", move || {
            let service = service.read().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for get",
            })?;
            service.get_skill(&skill_id)
        })
        .await
    }

    pub async fn download_skill(&self, skill_id: &str) -> Result<SkillDownload, SkillError> {
        let service = self.service.clone();
        let skill_id = skill_id.to_owned();
        run_skill_task("download skill", move || {
            let service = service.read().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for download",
            })?;
            service.download_skill(&skill_id)
        })
        .await
    }

    pub async fn list_skill_versions(
        &self,
        skill_id: &str,
    ) -> Result<Vec<SkillVersion>, SkillError> {
        let service = self.service.clone();
        let skill_id = skill_id.to_owned();
        run_skill_task("list skill versions", move || {
            let service = service.read().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for list versions",
            })?;
            service.list_skill_versions(&skill_id)
        })
        .await
    }

    pub async fn update_skill(
        &self,
        skill_id: &str,
        request: UpdateSkillRequest,
    ) -> Result<Skill, SkillError> {
        let service = self.service.clone();
        let skill_id = skill_id.to_owned();
        run_skill_task("update skill", move || {
            let service = service.write().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for update",
            })?;
            service.update_skill(&skill_id, &request.files)
        })
        .await
    }

    pub async fn rollback_skill_version(
        &self,
        skill_id: &str,
        version: u64,
    ) -> Result<Skill, SkillError> {
        let service = self.service.clone();
        let skill_id = skill_id.to_owned();
        run_skill_task("rollback skill version", move || {
            let service = service.write().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for rollback",
            })?;
            service.rollback_skill_version(&skill_id, version)
        })
        .await
    }

    pub async fn delete_skill(&self, skill_id: &str) -> Result<(), SkillError> {
        let service = self.service.clone();
        let skill_id = skill_id.to_owned();
        run_skill_task("delete skill", move || {
            let service = service.write().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for delete",
            })?;
            service.delete_skill(&skill_id)
        })
        .await
    }

    pub async fn resolve_volume_resources(
        &self,
        skill_ids: &[String],
    ) -> Result<Vec<SkillVolumeResource>, SkillError> {
        let service = self.service.clone();
        let skill_ids = skill_ids.to_vec();
        run_skill_task("resolve skill volume resources", move || {
            let service = service.read().map_err(|_| SkillError::LockPoisoned {
                operation: "lock skill service for resource resolution",
            })?;
            service.resolve_volume_resources(&skill_ids)
        })
        .await
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillMetadata {
    schema_version: u32,
    #[serde(default)]
    resources: SkillResources,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillResources {
    #[serde(default)]
    volumes: Vec<SkillVolumeDeclaration>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillVolumeDeclaration {
    id: String,
    scope: SkillVolumeScope,
    mount_path: String,
    #[serde(default)]
    advertise: bool,
    mode: SkillVolumeMode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum SkillVolumeScope {
    Installation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SkillVolumeMode {
    Ro,
    Rw,
}

impl Display for SkillVolumeMode {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Ro => "ro",
            Self::Rw => "rw",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillVolumeResource {
    pub skill_id: String,
    pub resource_id: String,
    pub mount_path: String,
    pub advertise: bool,
    pub mode: SkillVolumeMode,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Skill {
    pub skill_id: String,
    pub files: BTreeMap<String, String>,
    pub source: SkillSource,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillSummary {
    pub skill_id: String,
    pub source: SkillSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDownload {
    pub filename: String,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillVersion {
    pub skill_id: String,
    pub version: u64,
    pub created_at: String,
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateSkillRequest {
    pub skill_id: String,
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateSkillRequest {
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Builtin,
    User,
}

impl SkillSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
        }
    }
}

impl Display for SkillSource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub enum SkillError {
    SkillNotFound {
        skill_id: String,
    },
    SkillAlreadyExists {
        skill_id: String,
    },
    InvalidSkillId {
        skill_id: String,
    },
    InvalidSkillFilePath {
        path: String,
    },
    InvalidMetadata {
        skill_id: String,
        message: String,
    },
    BuiltinSkillReadOnly {
        skill_id: String,
    },
    SkillVersionNotFound {
        skill_id: String,
        version: u64,
    },
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Json {
        operation: &'static str,
        path: PathBuf,
        source: serde_json::Error,
    },
    Zip {
        operation: &'static str,
        source: zip::result::ZipError,
    },
    ArchiveIo {
        operation: &'static str,
        source: io::Error,
    },
    InvalidDownloadHeader {
        filename: String,
        source: header::InvalidHeaderValue,
    },
    PathPrefix {
        path: PathBuf,
        base: PathBuf,
        source: StripPrefixError,
    },
    LockPoisoned {
        operation: &'static str,
    },
    BlockingTaskJoin {
        operation: &'static str,
        source: tokio::task::JoinError,
    },
}

impl Display for SkillError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::SkillNotFound { skill_id } => write!(formatter, "skill not found: {skill_id}"),
            Self::SkillAlreadyExists { skill_id } => {
                write!(formatter, "skill already exists: {skill_id}")
            }
            Self::InvalidSkillId { skill_id } => write!(
                formatter,
                "skill_id must use lowercase alphanumeric characters and single hyphens only: \
                 {skill_id}"
            ),
            Self::InvalidSkillFilePath { path } => {
                write!(formatter, "invalid skill file path: {path}")
            }
            Self::InvalidMetadata { skill_id, message } => {
                write!(
                    formatter,
                    "invalid metadata for builtin skill '{skill_id}': {message}"
                )
            }
            Self::BuiltinSkillReadOnly { skill_id } => {
                write!(formatter, "builtin skill '{skill_id}' is read-only")
            }
            Self::SkillVersionNotFound { skill_id, version } => {
                write!(formatter, "skill version not found: {skill_id}@{version}")
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} at {}: {source}",
                path.display()
            ),
            Self::Json {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "failed to {operation} at {}: {source}",
                path.display()
            ),
            Self::Zip { operation, source } => write!(formatter, "failed to {operation}: {source}"),
            Self::ArchiveIo { operation, source } => {
                write!(formatter, "failed to {operation}: {source}")
            }
            Self::InvalidDownloadHeader { filename, source } => write!(
                formatter,
                "failed to build download header for {filename:?}: {source}"
            ),
            Self::PathPrefix { path, base, source } => write!(
                formatter,
                "failed to derive relative path for {} from {}: {source}",
                path.display(),
                base.display()
            ),
            Self::LockPoisoned { operation } => {
                write!(formatter, "failed to {operation}: lock poisoned")
            }
            Self::BlockingTaskJoin { operation, source } => {
                write!(
                    formatter,
                    "failed to {operation}: blocking task failed: {source}"
                )
            }
        }
    }
}

impl Error for SkillError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::ArchiveIo { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Zip { source, .. } => Some(source),
            Self::InvalidDownloadHeader { source, .. } => Some(source),
            Self::PathPrefix { source, .. } => Some(source),
            Self::BlockingTaskJoin { source, .. } => Some(source),
            Self::SkillNotFound { .. }
            | Self::SkillAlreadyExists { .. }
            | Self::InvalidSkillId { .. }
            | Self::InvalidSkillFilePath { .. }
            | Self::InvalidMetadata { .. }
            | Self::BuiltinSkillReadOnly { .. }
            | Self::SkillVersionNotFound { .. }
            | Self::LockPoisoned { .. } => None,
        }
    }
}

async fn run_skill_task<T>(
    operation: &'static str,
    task: impl FnOnce() -> Result<T, SkillError> + Send + 'static,
) -> Result<T, SkillError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|source| SkillError::BlockingTaskJoin { operation, source })?
}

#[derive(Clone, Debug)]
pub struct SkillsService {
    skills_dir: PathBuf,
    builtin_skills_dir: PathBuf,
    builtin_ids: BTreeSet<String>,
}

impl Default for SkillsService {
    fn default() -> Self {
        Self::from_env()
    }
}

impl SkillsService {
    #[must_use]
    pub fn from_env() -> Self {
        let skills_dir = env::var(ENV_SKILLS_DIR).unwrap_or_else(|_| DEFAULT_SKILLS_DIR.to_owned());
        let builtin_skills_dir = env::var(ENV_BUILTIN_SKILLS_DIR)
            .unwrap_or_else(|_| DEFAULT_BUILTIN_SKILLS_DIR.to_owned());

        Self::new(skills_dir, builtin_skills_dir)
    }

    #[must_use]
    pub fn new(skills_dir: impl Into<PathBuf>, builtin_skills_dir: impl Into<PathBuf>) -> Self {
        Self {
            skills_dir: skills_dir.into(),
            builtin_skills_dir: builtin_skills_dir.into(),
            builtin_ids: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }

    #[must_use]
    pub fn builtin_skills_dir(&self) -> &Path {
        &self.builtin_skills_dir
    }

    pub fn sync_builtin_skills(&mut self) -> Result<(), SkillError> {
        if !self.builtin_skills_dir.is_dir() {
            tracing::info!(
                path = %self.builtin_skills_dir.display(),
                "builtin skills dir not found, skipping sync"
            );
            return Ok(());
        }

        self.ensure_base_dir()?;
        let mut synced = Vec::new();

        for entry in read_dir_sorted(&self.builtin_skills_dir, "read builtin skills dir")? {
            if !entry_is_dir(&entry, "inspect builtin skill entry")? {
                continue;
            }

            let Some(skill_id) = entry.file_name().into_string().ok() else {
                tracing::warn!("skipping builtin skill with non-UTF-8 id");
                continue;
            };

            if validate_skill_id(&skill_id).is_err() {
                tracing::warn!(%skill_id, "skipping builtin skill with invalid id");
                continue;
            }

            Self::skill_volume_resources(&skill_id, &entry.path())?;
            let destination = self.skill_path(&skill_id);
            remove_existing_path(&destination, "remove existing builtin skill")?;
            copy_dir_all(&entry.path(), &destination)?;
            remove_existing_path(
                &self.skill_versions_dir(&skill_id),
                "remove builtin skill versions",
            )?;
            self.builtin_ids.insert(skill_id.clone());
            synced.push(skill_id);
        }

        tracing::info!(
            count = synced.len(),
            skills = ?synced,
            "synced builtin skills"
        );
        Ok(())
    }

    #[must_use]
    pub fn is_builtin(&self, skill_id: &str) -> bool {
        self.builtin_ids.contains(skill_id)
    }

    pub fn create_skill(
        &self,
        skill_id: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<Skill, SkillError> {
        validate_skill_id(skill_id)?;
        if self.is_builtin(skill_id) {
            return Err(SkillError::SkillAlreadyExists {
                skill_id: skill_id.to_owned(),
            });
        }

        self.ensure_base_dir()?;
        let skill_dir = self.skill_path(skill_id);
        if skill_dir.exists() {
            return Err(SkillError::SkillAlreadyExists {
                skill_id: skill_id.to_owned(),
            });
        }

        validate_file_paths(files.keys().map(String::as_str))?;
        fs::create_dir(&skill_dir)
            .map_err(|source| io_error("create skill dir", &skill_dir, source))?;
        write_skill_files(&skill_dir, files)?;
        self.save_skill_version(skill_id, files)?;

        tracing::info!(%skill_id, file_count = files.len(), "created skill");
        Ok(Skill {
            skill_id: skill_id.to_owned(),
            files: read_skill_files(&skill_dir)?,
            source: SkillSource::User,
        })
    }

    pub fn get_skill(&self, skill_id: &str) -> Result<Skill, SkillError> {
        validate_skill_id(skill_id)?;
        let skill_dir = self.skill_path(skill_id);
        if !skill_dir.is_dir() {
            return Err(SkillError::SkillNotFound {
                skill_id: skill_id.to_owned(),
            });
        }

        Ok(Skill {
            skill_id: skill_id.to_owned(),
            files: read_skill_files(&skill_dir)?,
            source: self.source_for(skill_id),
        })
    }

    pub fn download_skill(&self, skill_id: &str) -> Result<SkillDownload, SkillError> {
        let skill = self.get_skill(skill_id)?;
        if skill.files.len() == 1
            && let Some(content) = skill.files.get(SKILL_MARKDOWN_FILE)
        {
            return Ok(SkillDownload {
                filename: SKILL_MARKDOWN_FILE.to_owned(),
                content_type: MARKDOWN_CONTENT_TYPE,
                body: content.as_bytes().to_vec(),
            });
        }

        Ok(SkillDownload {
            filename: format!("{skill_id}.zip"),
            content_type: ZIP_CONTENT_TYPE,
            body: build_skill_zip(&skill.files)?,
        })
    }

    pub fn list_skills(&self) -> Result<Vec<SkillSummary>, SkillError> {
        self.ensure_base_dir()?;
        let mut skills = Vec::new();

        for entry in read_dir_sorted(&self.skills_dir, "read skills dir")? {
            if !entry_is_dir(&entry, "inspect skill entry")? {
                continue;
            }

            let Ok(skill_id) = entry.file_name().into_string() else {
                continue;
            };

            if validate_skill_id(&skill_id).is_ok() {
                skills.push(SkillSummary {
                    source: self.source_for(&skill_id),
                    skill_id,
                });
            }
        }

        Ok(skills)
    }

    fn resolve_volume_resources(
        &self,
        skill_ids: &[String],
    ) -> Result<Vec<SkillVolumeResource>, SkillError> {
        let mut resources = Vec::new();
        let mut mount_paths = BTreeSet::new();
        for skill_id in skill_ids {
            validate_skill_id(skill_id)?;
            if !self.is_builtin(skill_id) {
                continue;
            }
            for resource in Self::skill_volume_resources(skill_id, &self.skill_path(skill_id))? {
                if !mount_paths.insert(resource.mount_path.clone()) {
                    return Err(invalid_metadata(
                        skill_id,
                        format!(
                            "volume mount path {:?} conflicts with another enabled skill",
                            resource.mount_path
                        ),
                    ));
                }
                resources.push(resource);
            }
        }
        Ok(resources)
    }

    fn skill_volume_resources(
        skill_id: &str,
        skill_path: &Path,
    ) -> Result<Vec<SkillVolumeResource>, SkillError> {
        let metadata_path = skill_path.join(SKILL_METADATA_FILE);
        if !metadata_path.is_file() {
            return Ok(Vec::new());
        }
        let metadata_content = fs::read_to_string(&metadata_path)
            .map_err(|source| io_error("read skill metadata", &metadata_path, source))?;
        let metadata: SkillMetadata = serde_json::from_str(&metadata_content)
            .map_err(|source| json_error("parse skill metadata", &metadata_path, source))?;
        if metadata.schema_version != 1 {
            return Err(invalid_metadata(
                skill_id,
                format!(
                    "unsupported schema_version {}; expected 1",
                    metadata.schema_version
                ),
            ));
        }

        let mut resource_ids = BTreeSet::new();
        let mut mount_paths = BTreeSet::new();
        metadata
            .resources
            .volumes
            .into_iter()
            .map(|declaration| {
                validate_skill_id(&declaration.id).map_err(|_error| {
                    invalid_metadata(
                        skill_id,
                        format!("invalid volume resource id {:?}", declaration.id),
                    )
                })?;
                if !resource_ids.insert(declaration.id.clone()) {
                    return Err(invalid_metadata(
                        skill_id,
                        format!("duplicate volume resource id {:?}", declaration.id),
                    ));
                }
                validate_skill_mount_path(skill_id, &declaration.mount_path)?;
                if !mount_paths.insert(declaration.mount_path.clone()) {
                    return Err(invalid_metadata(
                        skill_id,
                        format!("duplicate volume mount_path {:?}", declaration.mount_path),
                    ));
                }
                let SkillVolumeScope::Installation = declaration.scope;
                Ok(SkillVolumeResource {
                    skill_id: skill_id.to_owned(),
                    resource_id: declaration.id,
                    mount_path: declaration.mount_path,
                    advertise: declaration.advertise,
                    mode: declaration.mode,
                })
            })
            .collect()
    }

    pub fn list_skill_versions(&self, skill_id: &str) -> Result<Vec<SkillVersion>, SkillError> {
        validate_skill_id(skill_id)?;
        self.ensure_user_skill_exists(skill_id)?;
        let versions_dir = self.skill_versions_dir(skill_id);
        if !versions_dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut versions = Vec::new();
        for entry in read_dir_sorted(&versions_dir, "read skill versions dir")? {
            let path = entry.path();
            if !entry
                .file_type()
                .map_err(|source| io_error("inspect skill version file", &path, source))?
                .is_file()
            {
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            versions.push(read_skill_version(&path)?);
        }
        versions.sort_by_key(|version| version.version);
        Ok(versions)
    }

    pub fn update_skill(
        &self,
        skill_id: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<Skill, SkillError> {
        validate_skill_id(skill_id)?;
        self.ensure_user_skill_exists(skill_id)?;

        let skill_dir = self.skill_path(skill_id);
        validate_file_paths(files.keys().map(String::as_str))?;
        if self.list_skill_versions(skill_id)?.is_empty() {
            let existing_files = read_skill_files(&skill_dir)?;
            self.save_skill_version(skill_id, &existing_files)?;
        }
        fs::remove_dir_all(&skill_dir)
            .map_err(|source| io_error("remove skill dir", &skill_dir, source))?;
        fs::create_dir_all(&skill_dir)
            .map_err(|source| io_error("create skill dir", &skill_dir, source))?;
        write_skill_files(&skill_dir, files)?;
        self.save_skill_version(skill_id, files)?;

        tracing::info!(%skill_id, file_count = files.len(), "updated skill");
        Ok(Skill {
            skill_id: skill_id.to_owned(),
            files: read_skill_files(&skill_dir)?,
            source: SkillSource::User,
        })
    }

    pub fn rollback_skill_version(
        &self,
        skill_id: &str,
        version: u64,
    ) -> Result<Skill, SkillError> {
        validate_skill_id(skill_id)?;
        self.ensure_user_skill_exists(skill_id)?;
        let snapshot_path = self.skill_version_path(skill_id, version);
        if !snapshot_path.is_file() {
            return Err(SkillError::SkillVersionNotFound {
                skill_id: skill_id.to_owned(),
                version,
            });
        }
        let snapshot = read_skill_version(&snapshot_path)?;
        self.update_skill(skill_id, &snapshot.files)
    }

    pub fn delete_skill(&self, skill_id: &str) -> Result<(), SkillError> {
        validate_skill_id(skill_id)?;
        if self.is_builtin(skill_id) {
            return Err(SkillError::BuiltinSkillReadOnly {
                skill_id: skill_id.to_owned(),
            });
        }

        let skill_dir = self.skill_path(skill_id);
        if !skill_dir.is_dir() {
            return Err(SkillError::SkillNotFound {
                skill_id: skill_id.to_owned(),
            });
        }

        fs::remove_dir_all(&skill_dir)
            .map_err(|source| io_error("remove skill dir", &skill_dir, source))?;
        remove_existing_path(
            &self.skill_versions_dir(skill_id),
            "remove skill version history",
        )?;
        tracing::info!(%skill_id, "deleted skill");
        Ok(())
    }

    fn ensure_base_dir(&self) -> Result<(), SkillError> {
        fs::create_dir_all(&self.skills_dir)
            .map_err(|source| io_error("create skills dir", &self.skills_dir, source))
    }

    fn skill_path(&self, skill_id: &str) -> PathBuf {
        self.skills_dir.join(skill_id)
    }

    fn skill_versions_dir(&self, skill_id: &str) -> PathBuf {
        self.skills_dir.join(SKILL_VERSIONS_DIR).join(skill_id)
    }

    fn skill_version_path(&self, skill_id: &str, version: u64) -> PathBuf {
        self.skill_versions_dir(skill_id)
            .join(format!("{version:020}.json"))
    }

    fn ensure_user_skill_exists(&self, skill_id: &str) -> Result<(), SkillError> {
        if self.is_builtin(skill_id) {
            return Err(SkillError::BuiltinSkillReadOnly {
                skill_id: skill_id.to_owned(),
            });
        }
        let skill_dir = self.skill_path(skill_id);
        if !skill_dir.is_dir() {
            return Err(SkillError::SkillNotFound {
                skill_id: skill_id.to_owned(),
            });
        }
        Ok(())
    }

    fn save_skill_version(
        &self,
        skill_id: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<SkillVersion, SkillError> {
        let versions_dir = self.skill_versions_dir(skill_id);
        fs::create_dir_all(&versions_dir)
            .map_err(|source| io_error("create skill versions dir", &versions_dir, source))?;
        let version = next_skill_version(&versions_dir)?;
        let snapshot = SkillVersion {
            skill_id: skill_id.to_owned(),
            version,
            created_at: utc_now(),
            files: files.clone(),
        };
        let path = self.skill_version_path(skill_id, version);
        let content = serde_json::to_vec_pretty(&snapshot)
            .map_err(|source| json_error("serialize skill version", &path, source))?;
        fs::write(&path, content)
            .map_err(|source| io_error("write skill version", &path, source))?;
        Ok(snapshot)
    }

    fn source_for(&self, skill_id: &str) -> SkillSource {
        if self.builtin_ids.contains(skill_id) {
            SkillSource::Builtin
        } else {
            SkillSource::User
        }
    }
}

fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn validate_skill_id(skill_id: &str) -> Result<(), SkillError> {
    let valid = !skill_id.is_empty()
        && skill_id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });

    if valid {
        Ok(())
    } else {
        Err(SkillError::InvalidSkillId {
            skill_id: skill_id.to_owned(),
        })
    }
}

fn validate_skill_mount_path(skill_id: &str, mount_path: &str) -> Result<(), SkillError> {
    const RESERVED_PATHS: &[&str] = &[
        "/workspace",
        "/mnt/all-skills",
        "/skills",
        "/root/.config/opencode/skills",
    ];
    let valid = mount_path.starts_with('/')
        && mount_path != "/"
        && !mount_path.ends_with('/')
        && mount_path
            .split('/')
            .skip(1)
            .all(|component| !component.is_empty() && component != "." && component != "..");
    if !valid {
        return Err(invalid_metadata(
            skill_id,
            format!("volume mount_path {mount_path:?} must be a normalized absolute path"),
        ));
    }
    if RESERVED_PATHS
        .iter()
        .any(|reserved| mount_path == *reserved || mount_path.starts_with(&format!("{reserved}/")))
    {
        return Err(invalid_metadata(
            skill_id,
            format!("volume mount_path {mount_path:?} overlaps a reserved kernel path"),
        ));
    }
    Ok(())
}

fn invalid_metadata(skill_id: &str, message: impl Into<String>) -> SkillError {
    SkillError::InvalidMetadata {
        skill_id: skill_id.to_owned(),
        message: message.into(),
    }
}

fn validate_file_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<(), SkillError> {
    for path in paths {
        validate_file_path(path)?;
    }
    Ok(())
}

fn validate_file_path(relative_path: &str) -> Result<(), SkillError> {
    let valid = !relative_path.is_empty()
        && !relative_path.starts_with('/')
        && !Path::new(relative_path).is_absolute()
        && relative_path
            .split('/')
            .try_fold(false, |has_file_component, component| {
                if component == ".." {
                    None
                } else {
                    Some(has_file_component || !(component.is_empty() || component == "."))
                }
            })
            .unwrap_or(false);

    if valid {
        Ok(())
    } else {
        Err(SkillError::InvalidSkillFilePath {
            path: relative_path.to_owned(),
        })
    }
}

fn write_skill_files(skill_dir: &Path, files: &BTreeMap<String, String>) -> Result<(), SkillError> {
    for (relative_path, content) in files {
        let file_path = skill_dir.join(normalize_posix_relative_path(relative_path));
        let parent = file_path
            .parent()
            .ok_or_else(|| SkillError::InvalidSkillFilePath {
                path: relative_path.clone(),
            })?;
        fs::create_dir_all(parent)
            .map_err(|source| io_error("create skill file parent dir", parent, source))?;
        fs::write(&file_path, content)
            .map_err(|source| io_error("write skill file", &file_path, source))?;
    }
    Ok(())
}

fn normalize_posix_relative_path(relative_path: &str) -> PathBuf {
    relative_path
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect()
}

fn read_skill_files(skill_dir: &Path) -> Result<BTreeMap<String, String>, SkillError> {
    let mut files = BTreeMap::new();
    collect_skill_files(skill_dir, skill_dir, &mut files)?;
    Ok(files)
}

fn build_skill_zip(files: &BTreeMap<String, String>) -> Result<Vec<u8>, SkillError> {
    let cursor = io::Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for (relative_path, content) in files {
        writer
            .start_file(relative_path, options)
            .map_err(|source| zip_error("start zip file", source))?;
        writer
            .write_all(content.as_bytes())
            .map_err(|source| archive_io_error("write zip file", source))?;
    }

    writer
        .finish()
        .map(io::Cursor::into_inner)
        .map_err(|source| zip_error("finish zip", source))
}

fn next_skill_version(versions_dir: &Path) -> Result<u64, SkillError> {
    let mut latest = 0;
    for entry in read_dir_sorted(versions_dir, "read skill versions dir")? {
        let path = entry.path();
        if !entry
            .file_type()
            .map_err(|source| io_error("inspect skill version file", &path, source))?
            .is_file()
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if let Ok(version) = stem.parse::<u64>() {
            latest = latest.max(version);
        }
    }
    Ok(latest + 1)
}

fn read_skill_version(path: &Path) -> Result<SkillVersion, SkillError> {
    let content =
        fs::read_to_string(path).map_err(|source| io_error("read skill version", path, source))?;
    serde_json::from_str(&content).map_err(|source| json_error("parse skill version", path, source))
}

fn collect_skill_files(
    base_dir: &Path,
    current_dir: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), SkillError> {
    for entry in read_dir_sorted(current_dir, "read skill files")? {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| io_error("inspect skill file", &path, source))?;

        if file_type.is_dir() {
            collect_skill_files(base_dir, &path, files)?;
        } else if file_type.is_file() {
            let relative_path = posix_relative_path(base_dir, &path)?;
            let content = fs::read_to_string(&path)
                .map_err(|source| io_error("read skill file", &path, source))?;
            files.insert(relative_path, content);
        }
    }

    Ok(())
}

fn posix_relative_path(base_dir: &Path, path: &Path) -> Result<String, SkillError> {
    let relative = path
        .strip_prefix(base_dir)
        .map_err(|source| SkillError::PathPrefix {
            path: path.to_path_buf(),
            base: base_dir.to_path_buf(),
            source,
        })?;

    relative
        .to_str()
        .map(|path| path.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| SkillError::InvalidSkillFilePath {
            path: relative.display().to_string(),
        })
}

fn copy_dir_all(source: &Path, destination: &Path) -> Result<(), SkillError> {
    fs::create_dir_all(destination)
        .map_err(|error| io_error("create builtin skill dir", destination, error))?;

    for entry in read_dir_sorted(source, "read builtin skill files")? {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| io_error("inspect builtin skill file", &source_path, error))?;

        if file_type.is_dir() {
            copy_dir_all(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path)
                .map_err(|error| io_error("copy builtin skill file", &source_path, error))?;
        }
    }

    Ok(())
}

fn remove_existing_path(path: &Path, operation: &'static str) -> Result<(), SkillError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).map_err(|source| io_error(operation, path, source))
        }
        Ok(_) => fs::remove_file(path).map_err(|source| io_error(operation, path, source)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(operation, path, error)),
    }
}

fn read_dir_sorted(path: &Path, operation: &'static str) -> Result<Vec<fs::DirEntry>, SkillError> {
    let mut entries = fs::read_dir(path)
        .map_err(|source| io_error(operation, path, source))?
        .collect::<Result<Vec<_>, io::Error>>()
        .map_err(|source| io_error(operation, path, source))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn entry_is_dir(entry: &fs::DirEntry, operation: &'static str) -> Result<bool, SkillError> {
    entry
        .file_type()
        .map(|file_type| file_type.is_dir())
        .map_err(|source| io_error(operation, &entry.path(), source))
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> SkillError {
    SkillError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

fn json_error(operation: &'static str, path: &Path, source: serde_json::Error) -> SkillError {
    SkillError::Json {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

const fn zip_error(operation: &'static str, source: zip::result::ZipError) -> SkillError {
    SkillError::Zip { operation, source }
}

const fn archive_io_error(operation: &'static str, source: io::Error) -> SkillError {
    SkillError::ArchiveIo { operation, source }
}

fn attachment_filename(filename: &str) -> String {
    format!("attachment; filename=\"{filename}\"")
}

fn skill_download_response(download: SkillDownload) -> Result<Response, SkillError> {
    let disposition =
        HeaderValue::from_str(&attachment_filename(&download.filename)).map_err(|source| {
            SkillError::InvalidDownloadHeader {
                filename: download.filename.clone(),
                source,
            }
        })?;
    let mut response = Body::from(download.body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(download.content_type),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition);
    Ok(response)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/skills", post(create_skill).get(list_skills))
        .route("/skills/{skill_id}/versions", get(list_skill_versions))
        .route(
            "/skills/{skill_id}/versions/{version}/rollback",
            post(rollback_skill_version),
        )
        .route("/skills/{skill_id}/download", get(download_skill))
        .route(
            "/skills/{skill_id}",
            get(get_skill).put(update_skill).delete(delete_skill),
        )
}

async fn create_skill(
    State(state): State<AppState>,
    Json(payload): Json<CreateSkillRequest>,
) -> Result<Json<Skill>, SkillHttpError> {
    state
        .skills
        .create_skill(payload)
        .await
        .map(Json)
        .map_err(SkillHttpError)
}

async fn list_skills(
    State(state): State<AppState>,
) -> Result<Json<Vec<SkillSummary>>, SkillHttpError> {
    state
        .skills
        .list_skills()
        .await
        .map(Json)
        .map_err(SkillHttpError)
}

async fn get_skill(
    State(state): State<AppState>,
    AxumPath(skill_id): AxumPath<String>,
) -> Result<Json<Skill>, SkillHttpError> {
    state
        .skills
        .get_skill(&skill_id)
        .await
        .map(Json)
        .map_err(SkillHttpError)
}

async fn download_skill(
    State(state): State<AppState>,
    AxumPath(skill_id): AxumPath<String>,
) -> Result<Response, SkillHttpError> {
    let download = state
        .skills
        .download_skill(&skill_id)
        .await
        .map_err(SkillHttpError)?;
    skill_download_response(download).map_err(SkillHttpError)
}

async fn list_skill_versions(
    State(state): State<AppState>,
    AxumPath(skill_id): AxumPath<String>,
) -> Result<Json<Vec<SkillVersion>>, SkillHttpError> {
    state
        .skills
        .list_skill_versions(&skill_id)
        .await
        .map(Json)
        .map_err(SkillHttpError)
}

async fn update_skill(
    State(state): State<AppState>,
    AxumPath(skill_id): AxumPath<String>,
    Json(payload): Json<UpdateSkillRequest>,
) -> Result<Json<Skill>, SkillHttpError> {
    state
        .skills
        .update_skill(&skill_id, payload)
        .await
        .map(Json)
        .map_err(SkillHttpError)
}

async fn rollback_skill_version(
    State(state): State<AppState>,
    AxumPath((skill_id, version)): AxumPath<(String, u64)>,
) -> Result<Json<Skill>, SkillHttpError> {
    state
        .skills
        .rollback_skill_version(&skill_id, version)
        .await
        .map(Json)
        .map_err(SkillHttpError)
}

async fn delete_skill(
    State(state): State<AppState>,
    AxumPath(skill_id): AxumPath<String>,
) -> Result<StatusCode, SkillHttpError> {
    state
        .skills
        .delete_skill(&skill_id)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(SkillHttpError)
}

#[derive(Debug)]
struct SkillHttpError(SkillError);

impl IntoResponse for SkillHttpError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            SkillError::SkillNotFound { .. } | SkillError::SkillVersionNotFound { .. } => {
                StatusCode::NOT_FOUND
            }
            SkillError::SkillAlreadyExists { .. } => StatusCode::CONFLICT,
            SkillError::InvalidSkillId { .. } | SkillError::InvalidSkillFilePath { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            SkillError::BuiltinSkillReadOnly { .. } => StatusCode::FORBIDDEN,
            SkillError::InvalidMetadata { .. }
            | SkillError::Io { .. }
            | SkillError::Json { .. }
            | SkillError::Zip { .. }
            | SkillError::ArchiveIo { .. }
            | SkillError::InvalidDownloadHeader { .. }
            | SkillError::PathPrefix { .. }
            | SkillError::LockPoisoned { .. }
            | SkillError::BlockingTaskJoin { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (status, Json(json!({ "detail": self.0.to_string() }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        io::{Cursor, Read},
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use axum::{
        Router,
        body::Body,
        http::{
            HeaderMap, Method, Request, StatusCode,
            header::{CONTENT_DISPOSITION, CONTENT_TYPE},
        },
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::{
        SkillError, SkillRegistry, SkillSource, SkillVolumeMode, SkillVolumeResource,
        SkillsService, validate_file_path, validate_skill_id,
    };
    use crate::{AppConfig, AppState, build_router};

    static NEXT_TEST_DIR_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let unique = format!(
                "{name}-{}-{}",
                process::id(),
                NEXT_TEST_DIR_ID.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::current_dir()
                .unwrap_or_else(|error| panic!("failed to read current dir: {error}"))
                .join("target")
                .join("agent_host_rs_skill_tests")
                .join(unique);

            if path.exists() {
                fs::remove_dir_all(&path)
                    .unwrap_or_else(|error| panic!("failed to clean test dir: {error}"));
            }
            fs::create_dir_all(&path)
                .unwrap_or_else(|error| panic!("failed to create test dir: {error}"));
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            match fs::remove_dir_all(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    panic!("failed to remove test dir {}: {error}", self.path.display());
                }
            }
        }
    }

    fn files(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
            .collect()
    }

    fn service(root: &TestDir) -> SkillsService {
        SkillsService::new(root.path().join("skills"), root.path().join("builtin"))
    }

    #[test]
    fn validates_skill_id_regex() {
        for skill_id in ["skill", "my-skill", "abc123", "a1-b2-c3"] {
            validate_skill_id(skill_id)
                .unwrap_or_else(|error| panic!("{skill_id} should be valid: {error}"));
        }

        for skill_id in [
            "",
            "Bad Skill",
            "../escape",
            "-leading",
            "trailing-",
            "double--hyphen",
            "under_score",
            "CAPS",
        ] {
            assert!(matches!(
                validate_skill_id(skill_id),
                Err(SkillError::InvalidSkillId { .. })
            ));
        }
    }

    #[test]
    fn validates_posix_relative_file_paths() {
        for path in ["SKILL.md", "tools/helper.py", "./docs//intro.md", "a/."] {
            validate_file_path(path)
                .unwrap_or_else(|error| panic!("{path} should be valid: {error}"));
        }

        for path in [
            "",
            "/",
            "/absolute.md",
            "../escape.md",
            "a/../escape.md",
            ".",
        ] {
            assert!(matches!(
                validate_file_path(path),
                Err(SkillError::InvalidSkillFilePath { .. })
            ));
        }
    }

    #[test]
    fn create_and_get_skill() {
        let root = TestDir::new("create-and-get");
        let service = service(&root);
        let created = service
            .create_skill(
                "my-skill",
                &files(&[
                    ("SKILL.md", "# My Skill\nDoes things."),
                    ("extra.md", "Extra info."),
                ]),
            )
            .unwrap_or_else(|error| panic!("failed to create skill: {error}"));

        assert_eq!(created.skill_id, "my-skill");
        assert_eq!(created.source, SkillSource::User);
        assert_eq!(
            created.files.get("SKILL.md").map(String::as_str),
            Some("# My Skill\nDoes things.")
        );
        assert_eq!(
            created.files.get("extra.md").map(String::as_str),
            Some("Extra info.")
        );

        let fetched = service
            .get_skill("my-skill")
            .unwrap_or_else(|error| panic!("failed to get skill: {error}"));
        assert_eq!(fetched, created);
    }

    #[test]
    fn download_single_file_skill_returns_skill_markdown() {
        let root = TestDir::new("download-markdown");
        let service = service(&root);
        service
            .create_skill("my-skill", &files(&[("SKILL.md", "# My Skill")]))
            .unwrap_or_else(|error| panic!("failed to create skill: {error}"));

        let download = service
            .download_skill("my-skill")
            .unwrap_or_else(|error| panic!("failed to prepare skill download: {error}"));

        assert_eq!(download.filename, "SKILL.md");
        assert_eq!(download.content_type, "text/markdown; charset=utf-8");
        assert_eq!(download.body.as_slice(), b"# My Skill");
    }

    #[test]
    fn download_multi_file_skill_returns_zip() {
        let root = TestDir::new("download-zip");
        let service = service(&root);
        service
            .create_skill(
                "my-skill",
                &files(&[
                    ("SKILL.md", "# My Skill"),
                    ("tools/helper.py", "print('hello')"),
                ]),
            )
            .unwrap_or_else(|error| panic!("failed to create skill: {error}"));

        let download = service
            .download_skill("my-skill")
            .unwrap_or_else(|error| panic!("failed to prepare skill download: {error}"));

        assert_eq!(download.filename, "my-skill.zip");
        assert_eq!(download.content_type, "application/zip");
        let entries = zip_entries(&download.body);
        assert_eq!(
            entries,
            BTreeMap::from([
                ("SKILL.md".to_owned(), "# My Skill".to_owned()),
                ("tools/helper.py".to_owned(), "print('hello')".to_owned()),
            ])
        );
    }

    #[test]
    fn list_skills_returns_sorted_valid_directories_only() {
        let root = TestDir::new("list-skills");
        let service = service(&root);
        service
            .create_skill("beta-skill", &files(&[("SKILL.md", "# Beta")]))
            .unwrap_or_else(|error| panic!("failed to create beta skill: {error}"));
        service
            .create_skill("alpha-skill", &files(&[("SKILL.md", "# Alpha")]))
            .unwrap_or_else(|error| panic!("failed to create alpha skill: {error}"));

        let invalid_dir = service.skills_dir().join("Bad Name");
        fs::create_dir_all(&invalid_dir)
            .unwrap_or_else(|error| panic!("failed to create invalid dir: {error}"));
        fs::write(service.skills_dir().join("not-a-dir"), "ignored")
            .unwrap_or_else(|error| panic!("failed to create ignored file: {error}"));

        let listed = service
            .list_skills()
            .unwrap_or_else(|error| panic!("failed to list skills: {error}"));

        assert_eq!(
            listed
                .iter()
                .map(|skill| skill.skill_id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha-skill", "beta-skill"]
        );
        assert!(listed.iter().all(|skill| skill.source == SkillSource::User));
    }

    #[test]
    fn update_skill_replaces_existing_files() {
        let root = TestDir::new("update-skill");
        let service = service(&root);
        service
            .create_skill(
                "my-skill",
                &files(&[("SKILL.md", "# V1"), ("old-file.md", "Old")]),
            )
            .unwrap_or_else(|error| panic!("failed to create skill: {error}"));

        let updated = service
            .update_skill(
                "my-skill",
                &files(&[("SKILL.md", "# V2"), ("new-file.md", "New content.")]),
            )
            .unwrap_or_else(|error| panic!("failed to update skill: {error}"));

        assert_eq!(
            updated.files.get("SKILL.md").map(String::as_str),
            Some("# V2")
        );
        assert_eq!(
            updated.files.get("new-file.md").map(String::as_str),
            Some("New content.")
        );
        assert_eq!(updated.files.len(), 2);
        assert!(!service.skills_dir().join("my-skill/old-file.md").exists());
    }

    #[test]
    fn user_skill_versions_are_recorded_and_rollback_creates_new_version() {
        let root = TestDir::new("versioned-skill");
        let service = service(&root);
        service
            .create_skill("my-skill", &files(&[("SKILL.md", "# V1")]))
            .unwrap_or_else(|error| panic!("failed to create skill: {error}"));
        service
            .update_skill(
                "my-skill",
                &files(&[("SKILL.md", "# V2"), ("notes.md", "second")]),
            )
            .unwrap_or_else(|error| panic!("failed to update skill: {error}"));

        let versions = service
            .list_skill_versions("my-skill")
            .unwrap_or_else(|error| panic!("failed to list versions: {error}"));
        assert_eq!(
            versions
                .iter()
                .map(|version| version.version)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            versions[0].files.get("SKILL.md").map(String::as_str),
            Some("# V1")
        );
        assert_eq!(
            versions[1].files.get("notes.md").map(String::as_str),
            Some("second")
        );

        let rolled_back = service
            .rollback_skill_version("my-skill", 1)
            .unwrap_or_else(|error| panic!("failed to rollback skill: {error}"));
        assert_eq!(
            rolled_back.files.get("SKILL.md").map(String::as_str),
            Some("# V1")
        );
        assert!(!rolled_back.files.contains_key("notes.md"));

        let versions = service
            .list_skill_versions("my-skill")
            .unwrap_or_else(|error| panic!("failed to list versions after rollback: {error}"));
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[2].version, 3);
        assert_eq!(
            versions[2].files.get("SKILL.md").map(String::as_str),
            Some("# V1")
        );
    }

    #[test]
    fn first_update_of_legacy_user_skill_snapshots_existing_files() {
        let root = TestDir::new("legacy-versioned-skill");
        let service = service(&root);
        write_file(
            &service.skills_dir().join("legacy-skill/SKILL.md"),
            "# Legacy",
        );
        write_file(
            &service.skills_dir().join("legacy-skill/notes.md"),
            "old notes",
        );
        assert!(
            service
                .list_skill_versions("legacy-skill")
                .unwrap_or_else(|error| panic!("failed to list initial versions: {error}"))
                .is_empty()
        );

        service
            .update_skill("legacy-skill", &files(&[("SKILL.md", "# Updated")]))
            .unwrap_or_else(|error| panic!("failed to update legacy skill: {error}"));

        let versions = service
            .list_skill_versions("legacy-skill")
            .unwrap_or_else(|error| panic!("failed to list versions: {error}"));
        assert_eq!(versions.len(), 2);
        assert_eq!(
            versions[0].files.get("SKILL.md").map(String::as_str),
            Some("# Legacy")
        );
        assert_eq!(
            versions[0].files.get("notes.md").map(String::as_str),
            Some("old notes")
        );
        assert_eq!(
            versions[1].files.get("SKILL.md").map(String::as_str),
            Some("# Updated")
        );

        let rolled_back = service
            .rollback_skill_version("legacy-skill", 1)
            .unwrap_or_else(|error| panic!("failed to rollback legacy skill: {error}"));
        assert_eq!(
            rolled_back.files.get("SKILL.md").map(String::as_str),
            Some("# Legacy")
        );
        assert_eq!(
            rolled_back.files.get("notes.md").map(String::as_str),
            Some("old notes")
        );
    }

    #[test]
    fn builtin_skills_do_not_expose_versions() {
        let root = TestDir::new("builtin-version-history");
        let mut service = service(&root);
        write_file(
            &service.builtin_skills_dir().join("websearch/SKILL.md"),
            "# Websearch",
        );
        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("failed to sync builtins: {error}"));

        assert!(matches!(
            service.list_skill_versions("websearch"),
            Err(SkillError::BuiltinSkillReadOnly { .. })
        ));
    }

    #[test]
    fn delete_skill_removes_skill() {
        let root = TestDir::new("delete-skill");
        let service = service(&root);
        service
            .create_skill("my-skill", &files(&[("SKILL.md", "# Doomed")]))
            .unwrap_or_else(|error| panic!("failed to create skill: {error}"));

        service
            .delete_skill("my-skill")
            .unwrap_or_else(|error| panic!("failed to delete skill: {error}"));

        assert!(matches!(
            service.get_skill("my-skill"),
            Err(SkillError::SkillNotFound { .. })
        ));
        assert!(
            service
                .list_skills()
                .unwrap_or_else(|error| panic!("failed to list skills: {error}"))
                .is_empty()
        );
    }

    #[test]
    fn create_duplicate_raises() {
        let root = TestDir::new("duplicate");
        let service = service(&root);
        service
            .create_skill("my-skill", &files(&[("SKILL.md", "# First")]))
            .unwrap_or_else(|error| panic!("failed to create skill: {error}"));

        assert!(matches!(
            service.create_skill("my-skill", &files(&[("SKILL.md", "# Second")])),
            Err(SkillError::SkillAlreadyExists { .. })
        ));
    }

    #[test]
    fn missing_skill_errors_are_typed() {
        let root = TestDir::new("missing");
        let service = service(&root);

        assert!(matches!(
            service.get_skill("nonexistent"),
            Err(SkillError::SkillNotFound { .. })
        ));
        assert!(matches!(
            service.update_skill("nonexistent", &files(&[("SKILL.md", "# Nope")])),
            Err(SkillError::SkillNotFound { .. })
        ));
        assert!(matches!(
            service.delete_skill("nonexistent"),
            Err(SkillError::SkillNotFound { .. })
        ));
    }

    #[test]
    fn invalid_skill_id_errors_are_typed() {
        let root = TestDir::new("invalid-id");
        let service = service(&root);

        assert!(matches!(
            service.create_skill("Bad Skill", &files(&[("SKILL.md", "# Bad")])),
            Err(SkillError::InvalidSkillId { .. })
        ));
        assert!(matches!(
            service.create_skill("../escape", &files(&[("SKILL.md", "# Bad")])),
            Err(SkillError::InvalidSkillId { .. })
        ));
    }

    #[test]
    fn invalid_file_path_errors_are_typed() {
        let root = TestDir::new("invalid-path");
        let service = service(&root);

        assert!(matches!(
            service.create_skill("my-skill", &files(&[("../escape.md", "# Bad")])),
            Err(SkillError::InvalidSkillFilePath { .. })
        ));
        assert!(matches!(
            service.create_skill("ok-skill", &files(&[("/absolute.md", "# Bad")])),
            Err(SkillError::InvalidSkillFilePath { .. })
        ));
        assert!(matches!(
            service.create_skill("empty-skill", &files(&[("", "# Bad")])),
            Err(SkillError::InvalidSkillFilePath { .. })
        ));
    }

    #[tokio::test]
    async fn skill_routes_match_python_lifecycle_contract() {
        let root = TestDir::new("skill-routes");
        let app = router_with_skills_service(service(&root));

        let (created_status, created) = json_request(
            &app,
            Method::POST,
            "/skills",
            json!({"skill_id": "my-skill", "files": {"SKILL.md": "# My Skill"}}),
        )
        .await;
        let (listed_status, listed) = empty_request(&app, Method::GET, "/skills").await;
        let (fetched_status, fetched) = empty_request(&app, Method::GET, "/skills/my-skill").await;
        let (download_status, download_headers, download_body) =
            binary_request(&app, Method::GET, "/skills/my-skill/download").await;
        let (updated_status, updated) = json_request(
            &app,
            Method::PUT,
            "/skills/my-skill",
            json!({"files": {"SKILL.md": "# Updated"}}),
        )
        .await;
        let (versions_status, versions) =
            empty_request(&app, Method::GET, "/skills/my-skill/versions").await;
        let (rollback_status, rollback) =
            empty_request(&app, Method::POST, "/skills/my-skill/versions/1/rollback").await;
        let deleted_status = status_request(&app, Method::DELETE, "/skills/my-skill").await;
        let after_status = status_request(&app, Method::GET, "/skills/my-skill").await;

        assert_eq!(created_status, StatusCode::OK);
        assert_eq!(created["skill_id"], "my-skill");
        assert_eq!(created["source"], "user");
        assert_eq!(listed_status, StatusCode::OK);
        assert_eq!(listed[0]["skill_id"], "my-skill");
        assert_eq!(fetched_status, StatusCode::OK);
        assert_eq!(fetched["files"]["SKILL.md"], "# My Skill");
        assert_eq!(download_status, StatusCode::OK);
        assert_eq!(
            download_headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/markdown; charset=utf-8")
        );
        assert_eq!(
            download_headers
                .get(CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok()),
            Some("attachment; filename=\"SKILL.md\"")
        );
        assert_eq!(download_body.as_slice(), b"# My Skill");
        assert_eq!(updated_status, StatusCode::OK);
        assert_eq!(updated["files"]["SKILL.md"], "# Updated");
        assert_eq!(versions_status, StatusCode::OK);
        assert_eq!(versions.as_array().map_or(0, Vec::len), 2);
        assert_eq!(versions[0]["files"]["SKILL.md"], "# My Skill");
        assert_eq!(rollback_status, StatusCode::OK);
        assert_eq!(rollback["files"]["SKILL.md"], "# My Skill");
        assert_eq!(deleted_status, StatusCode::NO_CONTENT);
        assert_eq!(after_status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn skill_routes_map_validation_conflict_and_builtin_errors() {
        let root = TestDir::new("skill-route-errors");
        write_file(
            &root.path().join("builtin/websearch/SKILL.md"),
            "# Websearch",
        );
        let registry = SkillRegistry::from_synced_service(service(&root))
            .unwrap_or_else(|error| panic!("failed to sync builtins: {error}"));
        let app = router_with_skill_registry(registry);

        let (duplicate_setup_status, _body) = json_request(
            &app,
            Method::POST,
            "/skills",
            json!({"skill_id": "dup-skill", "files": {"SKILL.md": "# First"}}),
        )
        .await;
        let duplicate_status = status_json_request(
            &app,
            Method::POST,
            "/skills",
            json!({"skill_id": "dup-skill", "files": {"SKILL.md": "# Second"}}),
        )
        .await;
        let invalid_id_status = status_json_request(
            &app,
            Method::POST,
            "/skills",
            json!({"skill_id": "Bad Skill", "files": {"SKILL.md": "# Bad"}}),
        )
        .await;
        let invalid_path_status = status_json_request(
            &app,
            Method::PUT,
            "/skills/dup-skill",
            json!({"files": {"../escape.md": "# Bad"}}),
        )
        .await;
        let builtin_update_status = status_json_request(
            &app,
            Method::PUT,
            "/skills/websearch",
            json!({"files": {"SKILL.md": "# Hacked"}}),
        )
        .await;
        let missing_status = status_request(&app, Method::GET, "/skills/missing-skill").await;

        assert_eq!(duplicate_setup_status, StatusCode::OK);
        assert_eq!(duplicate_status, StatusCode::CONFLICT);
        assert_eq!(invalid_id_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid_path_status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(builtin_update_status, StatusCode::FORBIDDEN);
        assert_eq!(missing_status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn nested_files_are_read_recursively() {
        let root = TestDir::new("nested");
        let service = service(&root);
        let created = service
            .create_skill(
                "nested-skill",
                &files(&[
                    ("SKILL.md", "# Nested Skill"),
                    ("tools/helper.py", "print('hello')"),
                    ("docs/intro.md", "Intro"),
                ]),
            )
            .unwrap_or_else(|error| panic!("failed to create skill: {error}"));

        assert_eq!(
            created.files.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["SKILL.md", "docs/intro.md", "tools/helper.py"]
        );
        assert_eq!(
            created.files.get("tools/helper.py").map(String::as_str),
            Some("print('hello')")
        );
    }

    #[test]
    fn sync_copies_builtin_skills_and_marks_source() {
        let root = TestDir::new("builtin-sync");
        let mut service = service(&root);
        write_file(
            &service.builtin_skills_dir().join("websearch/SKILL.md"),
            "# Websearch\nSearches the web.",
        );
        write_file(
            &service.builtin_skills_dir().join("websearch/search.sh"),
            "#!/bin/sh\ncurl $1",
        );
        write_file(
            &service.builtin_skills_dir().join("news/SKILL.md"),
            "# News\nFetches news.",
        );

        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("failed to sync builtins: {error}"));

        let listed = service
            .list_skills()
            .unwrap_or_else(|error| panic!("failed to list skills: {error}"));
        assert_eq!(
            listed
                .iter()
                .map(|skill| (skill.skill_id.as_str(), skill.source))
                .collect::<Vec<_>>(),
            vec![
                ("news", SkillSource::Builtin),
                ("websearch", SkillSource::Builtin)
            ]
        );

        let detail = service
            .get_skill("websearch")
            .unwrap_or_else(|error| panic!("failed to get builtin skill: {error}"));
        assert_eq!(detail.source, SkillSource::Builtin);
        assert!(detail.files.contains_key("SKILL.md"));
        assert!(detail.files.contains_key("search.sh"));
    }

    #[test]
    fn repository_memory_skill_declares_private_installation_volume() {
        let root = TestDir::new("repository-memory-skill");
        let builtin_skills_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mounts/skills");
        let mut service = SkillsService::new(root.path().join("skills"), builtin_skills_dir);

        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("failed to sync repository skills: {error}"));
        let resources = service
            .resolve_volume_resources(&["memory".to_owned()])
            .unwrap_or_else(|error| panic!("failed to resolve memory resources: {error}"));
        let memory_skill = service
            .get_skill("memory")
            .unwrap_or_else(|error| panic!("failed to read memory skill: {error}"));

        assert_eq!(
            resources,
            vec![SkillVolumeResource {
                skill_id: "memory".to_owned(),
                resource_id: "data".to_owned(),
                mount_path: "/var/lib/agentspace/memory".to_owned(),
                advertise: false,
                mode: SkillVolumeMode::Rw,
            }]
        );
        assert!(
            memory_skill
                .files
                .get("SKILL.md")
                .is_some_and(|content| !content.contains("/var/lib/agentspace/memory"))
        );
        assert!(memory_skill.files.contains_key("agentspace.json"));
    }

    #[test]
    fn builtin_sync_rejects_invalid_volume_metadata() {
        let cases = [
            (
                "schema",
                r#"{"schema_version":2,"resources":{"volumes":[]}}"#,
                "unsupported schema_version",
            ),
            (
                "duplicate-id",
                r#"{
                    "schema_version": 1,
                    "resources": {
                        "volumes": [
                            {"id":"data","scope":"installation","mount_path":"/data/a","mode":"rw"},
                            {"id":"data","scope":"installation","mount_path":"/data/b","mode":"rw"}
                        ]
                    }
                }"#,
                "duplicate volume resource id",
            ),
            (
                "duplicate-path",
                r#"{
                    "schema_version": 1,
                    "resources": {
                        "volumes": [
                            {"id":"one","scope":"installation","mount_path":"/data/shared","mode":"rw"},
                            {"id":"two","scope":"installation","mount_path":"/data/shared","mode":"ro"}
                        ]
                    }
                }"#,
                "duplicate volume mount_path",
            ),
            (
                "reserved-path",
                r#"{
                    "schema_version": 1,
                    "resources": {
                        "volumes": [
                            {"id":"data","scope":"installation","mount_path":"/workspace/memory","mode":"rw"}
                        ]
                    }
                }"#,
                "overlaps a reserved kernel path",
            ),
        ];

        for (name, metadata, expected) in cases {
            let root = TestDir::new(name);
            let mut service = service(&root);
            write_file(
                &service.builtin_skills_dir().join("invalid/SKILL.md"),
                "# Invalid",
            );
            write_file(
                &service.builtin_skills_dir().join("invalid/agentspace.json"),
                metadata,
            );

            let error = match service.sync_builtin_skills() {
                Ok(()) => panic!("invalid builtin metadata should fail startup sync"),
                Err(error) => error,
            };
            assert!(
                error.to_string().contains(expected),
                "{name}: expected {expected:?} in {error}"
            );
        }
    }

    #[test]
    fn enabled_builtin_volume_paths_must_not_collide() {
        let root = TestDir::new("enabled-volume-collision");
        let mut service = service(&root);
        let metadata = r#"{
            "schema_version": 1,
            "resources": {
                "volumes": [
                    {"id":"data","scope":"installation","mount_path":"/var/lib/shared","mode":"rw"}
                ]
            }
        }"#;
        for skill_id in ["first", "second"] {
            write_file(
                &service.builtin_skills_dir().join(skill_id).join("SKILL.md"),
                "# Skill",
            );
            write_file(
                &service
                    .builtin_skills_dir()
                    .join(skill_id)
                    .join("agentspace.json"),
                metadata,
            );
        }
        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("failed to sync valid builtins: {error}"));

        let Err(error) =
            service.resolve_volume_resources(&["first".to_owned(), "second".to_owned()])
        else {
            panic!("colliding enabled skill volumes should fail");
        };
        assert!(
            error
                .to_string()
                .contains("conflicts with another enabled skill")
        );
    }

    #[test]
    fn builtin_skills_are_read_only_and_duplicate_create_is_rejected() {
        let root = TestDir::new("builtin-read-only");
        let mut service = service(&root);
        write_file(
            &service.builtin_skills_dir().join("websearch/SKILL.md"),
            "# Websearch",
        );
        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("failed to sync builtins: {error}"));

        assert!(matches!(
            service.update_skill("websearch", &files(&[("SKILL.md", "# Hacked")])),
            Err(SkillError::BuiltinSkillReadOnly { .. })
        ));
        assert!(matches!(
            service.delete_skill("websearch"),
            Err(SkillError::BuiltinSkillReadOnly { .. })
        ));
        assert!(matches!(
            service.create_skill("websearch", &files(&[("SKILL.md", "# Dup")])),
            Err(SkillError::SkillAlreadyExists { .. })
        ));
    }

    #[test]
    fn user_skills_can_exist_alongside_builtins() {
        let root = TestDir::new("builtin-and-user");
        let mut service = service(&root);
        write_file(
            &service.builtin_skills_dir().join("news/SKILL.md"),
            "# News",
        );
        write_file(
            &service.builtin_skills_dir().join("websearch/SKILL.md"),
            "# Websearch",
        );
        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("failed to sync builtins: {error}"));
        service
            .create_skill("my-custom", &files(&[("SKILL.md", "# Custom")]))
            .unwrap_or_else(|error| panic!("failed to create user skill: {error}"));

        let sources = service
            .list_skills()
            .unwrap_or_else(|error| panic!("failed to list skills: {error}"))
            .into_iter()
            .map(|skill| (skill.skill_id, skill.source))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(sources.get("websearch"), Some(&SkillSource::Builtin));
        assert_eq!(sources.get("news"), Some(&SkillSource::Builtin));
        assert_eq!(sources.get("my-custom"), Some(&SkillSource::User));
    }

    #[test]
    fn sync_overwrites_existing_skill() {
        let root = TestDir::new("builtin-overwrite");
        let mut service = service(&root);
        write_file(
            &service.skills_dir().join("websearch/SKILL.md"),
            "# Old version",
        );
        write_file(
            &service.builtin_skills_dir().join("websearch/SKILL.md"),
            "# New version",
        );

        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("failed to sync builtins: {error}"));

        let detail = service
            .get_skill("websearch")
            .unwrap_or_else(|error| panic!("failed to get builtin skill: {error}"));
        assert_eq!(
            detail.files.get("SKILL.md").map(String::as_str),
            Some("# New version")
        );
        assert_eq!(detail.source, SkillSource::Builtin);
    }

    #[test]
    fn sync_skips_invalid_dir_names() {
        let root = TestDir::new("builtin-invalid");
        let mut service = service(&root);
        write_file(
            &service.builtin_skills_dir().join("Bad Name/SKILL.md"),
            "# Bad",
        );

        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("failed to sync builtins: {error}"));

        assert!(
            service
                .list_skills()
                .unwrap_or_else(|error| panic!("failed to list skills: {error}"))
                .is_empty()
        );
    }

    #[test]
    fn sync_with_missing_builtin_dir_is_ok() {
        let root = TestDir::new("builtin-missing");
        let mut service = service(&root);

        service
            .sync_builtin_skills()
            .unwrap_or_else(|error| panic!("missing builtin dir should be ok: {error}"));

        assert!(
            service
                .list_skills()
                .unwrap_or_else(|error| panic!("failed to list skills: {error}"))
                .is_empty()
        );
    }

    fn router_with_skills_service(service: SkillsService) -> Router {
        router_with_skill_registry(SkillRegistry::new(service))
    }

    fn router_with_skill_registry(registry: SkillRegistry) -> Router {
        let state = AppState::with_skill_registry(
            AppConfig::new("127.0.0.1", 0, BTreeMap::new()),
            registry,
        );
        build_router(state)
    }

    async fn json_request(
        app: &Router,
        method: Method,
        uri: &str,
        payload: Value,
    ) -> (StatusCode, Value) {
        let body = serde_json::to_vec(&payload)
            .unwrap_or_else(|error| panic!("failed to serialize request body: {error}"));
        request(app, method, uri, Body::from(body), true).await
    }

    async fn status_json_request(
        app: &Router,
        method: Method,
        uri: &str,
        payload: Value,
    ) -> StatusCode {
        let (status, _body) = json_request(app, method, uri, payload).await;
        status
    }

    async fn empty_request(app: &Router, method: Method, uri: &str) -> (StatusCode, Value) {
        request(app, method, uri, Body::empty(), false).await
    }

    async fn status_request(app: &Router, method: Method, uri: &str) -> StatusCode {
        let (status, _body) = empty_request(app, method, uri).await;
        status
    }

    async fn binary_request(
        app: &Router,
        method: Method,
        uri: &str,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));
        let response = app
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
            .to_bytes()
            .to_vec();

        (status, headers, body)
    }

    async fn request(
        app: &Router,
        method: Method,
        uri: &str,
        body: Body,
        json_body: bool,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if json_body {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        let request = builder
            .body(body)
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));
        let response = app
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|error| panic!("failed to read response body: {error}"))
            .to_bytes();
        let payload = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("failed to parse response body: {error}"))
        };

        (status, payload)
    }

    fn zip_entries(content: &[u8]) -> BTreeMap<String, String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(content))
            .unwrap_or_else(|error| panic!("failed to read zip archive: {error}"));
        let mut entries = BTreeMap::new();
        for index in 0..archive.len() {
            let mut file = archive
                .by_index(index)
                .unwrap_or_else(|error| panic!("failed to read zip entry {index}: {error}"));
            let mut content = String::new();
            file.read_to_string(&mut content)
                .unwrap_or_else(|error| panic!("failed to read zip entry content: {error}"));
            entries.insert(file.name().to_owned(), content);
        }
        entries
    }

    fn write_file(path: &Path, content: &str) {
        let parent = path
            .parent()
            .unwrap_or_else(|| panic!("test path should have a parent: {}", path.display()));
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", parent.display()));
        fs::write(path, content)
            .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
    }
}
