use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    io,
    path::PathBuf,
};

use crate::{api::ApiError, environment::EnvironmentError};

#[derive(Debug)]
pub enum SkillsError {
    Environment(EnvironmentError),
    Api(ApiError),
    InvalidSkillDirectory { path: PathBuf },
    InvalidSkillId { skill_id: String },
    MissingManifest { path: PathBuf },
    Symlink { path: PathBuf },
    UnsupportedFile { path: PathBuf },
    NonUtf8Path { path: PathBuf },
    NonUtf8Content { path: PathBuf },
    BuiltinReadOnly { skill_id: String },
    Io { path: PathBuf, source: io::Error },
    Json { source: serde_json::Error },
}

impl SkillsError {
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::InvalidSkillDirectory { .. }
            | Self::InvalidSkillId { .. }
            | Self::MissingManifest { .. }
            | Self::Symlink { .. }
            | Self::UnsupportedFile { .. }
            | Self::NonUtf8Path { .. }
            | Self::NonUtf8Content { .. }
            | Self::BuiltinReadOnly { .. }
            | Self::Environment(_) => 2,
            Self::Api(ApiError::Response { status, .. }) if status.as_u16() == 404 => 3,
            Self::Api(ApiError::Response { status, .. }) if status.as_u16() == 409 => 4,
            Self::Api(ApiError::Unavailable { .. } | ApiError::Timeout { .. }) => 10,
            Self::Api(_) | Self::Io { .. } | Self::Json { .. } => 1,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Environment(_) => "configuration",
            Self::Api(ApiError::Response { status, .. }) if status.as_u16() == 404 => "not_found",
            Self::Api(ApiError::Response { status, .. }) if status.as_u16() == 409 => "conflict",
            Self::Api(ApiError::Unavailable { .. }) => "unavailable",
            Self::Api(ApiError::Timeout { .. }) => "timeout",
            Self::Api(_) => "api",
            Self::InvalidSkillDirectory { .. }
            | Self::InvalidSkillId { .. }
            | Self::MissingManifest { .. }
            | Self::Symlink { .. }
            | Self::UnsupportedFile { .. }
            | Self::NonUtf8Path { .. }
            | Self::NonUtf8Content { .. }
            | Self::BuiltinReadOnly { .. } => "invalid_skill",
            Self::Io { .. } => "io",
            Self::Json { .. } => "json",
        }
    }
}

impl Display for SkillsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(error) => Display::fmt(error, formatter),
            Self::Api(error) => Display::fmt(error, formatter),
            Self::InvalidSkillDirectory { path } => {
                write!(
                    formatter,
                    "skill directory does not exist: {}",
                    path.display()
                )
            }
            Self::InvalidSkillId { skill_id } => write!(
                formatter,
                "skill directory name must use lowercase alphanumeric characters and single \
                 hyphens only: {skill_id}"
            ),
            Self::MissingManifest { path } => {
                write!(formatter, "skill directory must contain {}", path.display())
            }
            Self::Symlink { path } => {
                write!(
                    formatter,
                    "skill content cannot contain symlinks: {}",
                    path.display()
                )
            }
            Self::UnsupportedFile { path } => {
                write!(formatter, "unsupported skill file type: {}", path.display())
            }
            Self::NonUtf8Path { path } => {
                write!(
                    formatter,
                    "skill file path is not UTF-8: {}",
                    path.display()
                )
            }
            Self::NonUtf8Content { path } => {
                write!(
                    formatter,
                    "skill file is not UTF-8 text: {}",
                    path.display()
                )
            }
            Self::BuiltinReadOnly { skill_id } => {
                write!(formatter, "builtin skill {skill_id:?} is read-only")
            }
            Self::Io { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::Json { source } => write!(formatter, "failed to serialize output: {source}"),
        }
    }
}

impl Error for SkillsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Environment(error) => Some(error),
            Self::Api(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::Json { source } => Some(source),
            Self::InvalidSkillDirectory { .. }
            | Self::InvalidSkillId { .. }
            | Self::MissingManifest { .. }
            | Self::Symlink { .. }
            | Self::UnsupportedFile { .. }
            | Self::NonUtf8Path { .. }
            | Self::NonUtf8Content { .. }
            | Self::BuiltinReadOnly { .. } => None,
        }
    }
}

impl From<ApiError> for SkillsError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<EnvironmentError> for SkillsError {
    fn from(error: EnvironmentError) -> Self {
        Self::Environment(error)
    }
}
