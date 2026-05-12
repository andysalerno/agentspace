use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    fs, io,
    path::{Path, PathBuf, StripPrefixError},
    sync::{Arc, RwLock},
};

use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{AppState, models::ServiceSummary};

const ENV_SKILLS_DIR: &str = "AGENT_HOST_SKILLS_DIR";
const ENV_BUILTIN_SKILLS_DIR: &str = "AGENT_HOST_BUILTIN_SKILLS_DIR";
const DEFAULT_SKILLS_DIR: &str = "/skills";
const DEFAULT_BUILTIN_SKILLS_DIR: &str = "/builtin-skills";

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
    BuiltinSkillReadOnly {
        skill_id: String,
    },
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
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
            Self::BuiltinSkillReadOnly { skill_id } => {
                write!(formatter, "builtin skill '{skill_id}' is read-only")
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
            Self::Io { source, .. } => Some(source),
            Self::PathPrefix { source, .. } => Some(source),
            Self::BlockingTaskJoin { source, .. } => Some(source),
            Self::SkillNotFound { .. }
            | Self::SkillAlreadyExists { .. }
            | Self::InvalidSkillId { .. }
            | Self::InvalidSkillFilePath { .. }
            | Self::BuiltinSkillReadOnly { .. }
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

            let destination = self.skill_path(&skill_id);
            remove_existing_path(&destination, "remove existing builtin skill")?;
            copy_dir_all(&entry.path(), &destination)?;
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

    pub fn update_skill(
        &self,
        skill_id: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<Skill, SkillError> {
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

        validate_file_paths(files.keys().map(String::as_str))?;
        fs::remove_dir_all(&skill_dir)
            .map_err(|source| io_error("remove skill dir", &skill_dir, source))?;
        fs::create_dir_all(&skill_dir)
            .map_err(|source| io_error("create skill dir", &skill_dir, source))?;
        write_skill_files(&skill_dir, files)?;

        tracing::info!(%skill_id, file_count = files.len(), "updated skill");
        Ok(Skill {
            skill_id: skill_id.to_owned(),
            files: read_skill_files(&skill_dir)?,
            source: SkillSource::User,
        })
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

    fn source_for(&self, skill_id: &str) -> SkillSource {
        if self.builtin_ids.contains(skill_id) {
            SkillSource::Builtin
        } else {
            SkillSource::User
        }
    }
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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/skills", post(create_skill).get(list_skills))
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
            SkillError::SkillNotFound { .. } => StatusCode::NOT_FOUND,
            SkillError::SkillAlreadyExists { .. } => StatusCode::CONFLICT,
            SkillError::InvalidSkillId { .. } | SkillError::InvalidSkillFilePath { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            SkillError::BuiltinSkillReadOnly { .. } => StatusCode::FORBIDDEN,
            SkillError::Io { .. }
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
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use axum::{
        Router,
        body::Body,
        http::{Method, Request, StatusCode, header::CONTENT_TYPE},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::{
        SkillError, SkillRegistry, SkillSource, SkillsService, validate_file_path,
        validate_skill_id,
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
        let (updated_status, updated) = json_request(
            &app,
            Method::PUT,
            "/skills/my-skill",
            json!({"files": {"SKILL.md": "# Updated"}}),
        )
        .await;
        let deleted_status = status_request(&app, Method::DELETE, "/skills/my-skill").await;
        let after_status = status_request(&app, Method::GET, "/skills/my-skill").await;

        assert_eq!(created_status, StatusCode::OK);
        assert_eq!(created["skill_id"], "my-skill");
        assert_eq!(created["source"], "user");
        assert_eq!(listed_status, StatusCode::OK);
        assert_eq!(listed[0]["skill_id"], "my-skill");
        assert_eq!(fetched_status, StatusCode::OK);
        assert_eq!(fetched["files"]["SKILL.md"], "# My Skill");
        assert_eq!(updated_status, StatusCode::OK);
        assert_eq!(updated["files"]["SKILL.md"], "# Updated");
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
