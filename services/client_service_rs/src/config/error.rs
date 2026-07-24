use std::fmt::{self, Display, Formatter};

/// A single, structured validation issue. Field paths use a stable
/// `kind/name/field` shape so clients can render actionable errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationIssue {
    pub code: String,
    pub detail: String,
    #[allow(clippy::struct_field_names)]
    pub resource: Option<String>,
    pub field: Option<String>,
}

impl ValidationIssue {
    #[must_use]
    pub fn new(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            resource: None,
            field: None,
        }
    }

    #[must_use]
    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    #[must_use]
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
}

/// Errors produced while loading, validating, serializing, or applying config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    /// The YAML/document could not be parsed or violated strict schema rules.
    Parse { detail: String },
    /// The document referenced an unsupported `apiVersion`.
    UnsupportedApiVersion { value: String },
    /// The document referenced an unsupported `kind`.
    UnsupportedKind { value: String },
    /// A resource identity appeared more than once in the config set.
    DuplicateResource { kind: String, id: String },
    /// A bundle/config-set was submitted but bundle apply is not implemented.
    UnsupportedBundle,
    /// A config-set bundle could not be read or violated a safety limit.
    Bundle { detail: String },
    /// One or more graph/reference validation issues.
    Validation { issues: Vec<ValidationIssue> },
    /// Canonical serialization failed.
    Serialize { detail: String },
    /// An apply omitted a declaration whose value is set (409).
    SecretDeclarationRemovalBlocked { names: Vec<String> },
    /// The canonical projection did not round-trip to an equal typed value.
    CanonicalDrift,
    /// An optimistic-concurrency apply observed a different active generation
    /// than the caller expected (409).
    GenerationConflict { expected: i64, actual: Option<i64> },
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse { detail } => write!(formatter, "failed to parse configuration: {detail}"),
            Self::UnsupportedApiVersion { value } => {
                write!(formatter, "unsupported apiVersion {value:?}")
            }
            Self::UnsupportedKind { value } => write!(formatter, "unsupported kind {value:?}"),
            Self::DuplicateResource { kind, id } => {
                write!(formatter, "duplicate {kind} identity {id:?}")
            }
            Self::UnsupportedBundle => {
                write!(
                    formatter,
                    "config-set bundles are not supported yet; submit a single YAML document"
                )
            }
            Self::Bundle { detail } => {
                write!(formatter, "invalid config-set bundle: {detail}")
            }
            Self::Validation { issues } => {
                write!(formatter, "configuration is invalid: ")?;
                for (index, issue) in issues.iter().enumerate() {
                    if index > 0 {
                        write!(formatter, "; ")?;
                    }
                    write!(formatter, "{}", issue.detail)?;
                }
                Ok(())
            }
            Self::Serialize { detail } => {
                write!(formatter, "failed to serialize configuration: {detail}")
            }
            Self::SecretDeclarationRemovalBlocked { names } => write!(
                formatter,
                "cannot remove secret declaration(s) with set values: {}",
                names.join(", ")
            ),
            Self::CanonicalDrift => write!(
                formatter,
                "canonical projection did not round-trip to an equal document"
            ),
            Self::GenerationConflict { expected, actual } => write!(
                formatter,
                "configuration generation conflict: expected active generation {expected}, but the active generation is {}",
                actual.map_or_else(|| "none".to_owned(), |value| value.to_string())
            ),
        }
    }
}

impl std::error::Error for ConfigError {}
