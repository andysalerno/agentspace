//! JSON wire models shared by the Axum adapter ([`crate::server`]) and the
//! HTTP transport ([`crate::http_client`]).
//!
//! Every `/v1/...` JSON endpoint (every route except a successful
//! `/v1/run`, which streams the framed protocol documented in
//! [`crate::run_stream`]) exchanges exactly the request/response/error
//! shapes defined here, so both sides of the wire stay in lockstep and
//! neither transport re-derives the mapping independently.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Mapping;

use crate::{
    error::MemoryError,
    model::{Page, PageLink, PageMetadata, Revision},
    path::PagePath,
};

/// The JSON content type used by every `/v1/...` request and response body
/// except a successful `/v1/run` stream.
pub const JSON_CONTENT_TYPE: &str = "application/json";

/// A full page as returned by `GET /v1/pages/content`: metadata, body,
/// outgoing links, and revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageWire {
    pub path: String,
    pub schema_version: u64,
    pub title: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub extra: Mapping,
    pub revision: String,
    pub body: String,
    pub outgoing_links: Vec<PageLink>,
}

impl PageWire {
    #[must_use]
    pub fn from_page(page: &Page, outgoing_links: Vec<PageLink>) -> Self {
        Self {
            path: page.path.as_str(),
            schema_version: page.metadata.schema_version,
            title: page.metadata.title.clone(),
            tags: page.metadata.tags.clone(),
            created_at: page.metadata.created_at,
            updated_at: page.metadata.updated_at,
            created_by: page.metadata.created_by.clone(),
            updated_by: page.metadata.updated_by.clone(),
            extra: page.metadata.extra.clone(),
            revision: page.revision.0.clone(),
            body: page.body.clone(),
            outgoing_links,
        }
    }
}

impl TryFrom<PageWire> for Page {
    type Error = MemoryError;

    fn try_from(wire: PageWire) -> Result<Self, Self::Error> {
        Ok(Self {
            path: PagePath::parse(&wire.path)?,
            metadata: PageMetadata {
                schema_version: wire.schema_version,
                title: wire.title,
                tags: wire.tags,
                created_at: wire.created_at,
                updated_at: wire.updated_at,
                created_by: wire.created_by,
                updated_by: wire.updated_by,
                extra: wire.extra,
            },
            body: wire.body,
            revision: Revision(wire.revision),
        })
    }
}

/// Query parameters accepted by `GET /v1/pages`, mirroring `memory pages ls`
/// and `memory query`.
///
/// A repeatable CLI flag (`--with-tag`) is represented on the wire as a
/// single comma-separated value rather than a repeated query key, since
/// `Query` extraction here uses `serde_urlencoded`, which does not reliably
/// collect repeated keys into a sequence.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListPagesQuery {
    pub under: Option<String>,
    #[serde(default, rename = "with-tag")]
    pub with_tag: Option<String>,
    pub limit: Option<usize>,
    /// When present and non-empty, performs `memory query` instead of
    /// `memory pages ls`.
    pub text: Option<String>,
}

impl ListPagesQuery {
    #[must_use]
    pub fn tags(&self) -> Vec<String> {
        self.with_tag
            .as_deref()
            .map(|tags| {
                tags.split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Query parameters accepted by `GET`/`PUT`/`DELETE /v1/pages/content`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentQuery {
    pub path: String,
    /// Only observed by `DELETE`; mutations elsewhere carry
    /// `expected_revision` in their JSON body.
    pub expected_revision: Option<String>,
}

/// Query parameters accepted by `GET /v1/links`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinksQuery {
    pub path: String,
    #[serde(default)]
    pub backlinks: bool,
}

/// JSON body accepted by `PUT /v1/pages/content`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WritePageWire {
    pub title: Option<String>,
    pub tags: Option<Vec<String>>,
    pub body: String,
    #[serde(default)]
    pub overwrite: bool,
    pub expected_revision: Option<String>,
    pub actor: Option<String>,
}

/// JSON body accepted by `POST /v1/pages/move`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MovePageWire {
    pub source: String,
    pub destination: String,
    pub expected_revision: Option<String>,
    pub actor: Option<String>,
}

/// JSON body accepted by `POST /v1/run`.
///
/// Clients may request a smaller timeout or output cap than the server
/// default, but the server always clamps both to its own configured
/// maximums; it never grants a caller a wider bound.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunRequestWire {
    pub argv: Vec<String>,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
}

/// The stable JSON error envelope returned by every non-success `/v1/...`
/// response.
///
/// This includes a failed `/v1/run` (a `/v1/run` request that never starts
/// streaming, e.g. a disallowed command or a launch failure).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// The structured contents of [`ErrorEnvelope`].
///
/// `kind` is the same stable string produced by [`MemoryError::kind`]; the
/// optional fields recover enough detail for a remote transport to
/// reconstruct an equivalent [`MemoryError`] rather than collapsing every
/// failure into an opaque message string.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl ErrorEnvelope {
    #[must_use]
    pub fn from_error(error: &MemoryError) -> Self {
        let message = error.to_string();
        let kind = error.kind().to_owned();
        let body = match error {
            MemoryError::InvalidPath { path, .. }
            | MemoryError::NotFound { path }
            | MemoryError::AlreadyExists { path }
            | MemoryError::InvalidFrontmatter { path, .. } => ErrorBody {
                kind,
                message,
                path: Some(path.clone()),
                expected_revision: None,
                actual_revision: None,
                limit: None,
                command: None,
            },
            MemoryError::Conflict {
                path,
                expected,
                actual,
            } => ErrorBody {
                kind,
                message,
                path: Some(path.clone()),
                expected_revision: expected.clone(),
                actual_revision: Some(actual.clone()),
                limit: None,
                command: None,
            },
            MemoryError::TooLarge { limit, .. } => ErrorBody {
                kind,
                message,
                path: None,
                expected_revision: None,
                actual_revision: None,
                limit: Some(*limit),
                command: None,
            },
            MemoryError::CommandNotAllowed { command } => ErrorBody {
                kind,
                message,
                path: None,
                expected_revision: None,
                actual_revision: None,
                limit: None,
                command: Some(command.clone()),
            },
            MemoryError::RunTimedOut
            | MemoryError::RunOutputLimitExceeded
            | MemoryError::RunCancelled
            | MemoryError::RunLaunchFailed { .. }
            | MemoryError::Lock { .. }
            | MemoryError::NotImplemented { .. }
            | MemoryError::Unavailable { .. }
            | MemoryError::MalformedResponse { .. }
            | MemoryError::Internal { .. }
            | MemoryError::Io { .. }
            | MemoryError::Yaml { .. } => ErrorBody {
                kind,
                message,
                path: None,
                expected_revision: None,
                actual_revision: None,
                limit: None,
                command: None,
            },
        };
        Self { error: body }
    }

    /// Reconstructs a [`MemoryError`] from a parsed error response, using
    /// `kind` to select the variant and falling back to
    /// [`MemoryError::Internal`] for a kind this build does not recognize
    /// (e.g. a newer server version), so an unfamiliar failure is still
    /// reported rather than silently treated as success.
    #[must_use]
    pub fn into_memory_error(self) -> MemoryError {
        let ErrorBody {
            kind,
            message,
            path,
            expected_revision,
            actual_revision,
            limit,
            command,
        } = self.error;
        match kind.as_str() {
            "invalid_path" => MemoryError::InvalidPath {
                path: path.unwrap_or_default(),
                reason: message,
            },
            "not_found" => MemoryError::NotFound {
                path: path.unwrap_or_default(),
            },
            "already_exists" => MemoryError::AlreadyExists {
                path: path.unwrap_or_default(),
            },
            "conflict" => MemoryError::Conflict {
                path: path.unwrap_or_default(),
                expected: expected_revision,
                actual: actual_revision.unwrap_or_default(),
            },
            "invalid_frontmatter" => MemoryError::InvalidFrontmatter {
                path: path.unwrap_or_default(),
                reason: message,
            },
            "too_large" => MemoryError::TooLarge {
                what: "request payload",
                limit: limit.unwrap_or_default(),
            },
            "command_not_allowed" => MemoryError::CommandNotAllowed {
                command: command.unwrap_or_default(),
            },
            "run_timed_out" => MemoryError::RunTimedOut,
            "run_output_limit_exceeded" => MemoryError::RunOutputLimitExceeded,
            "run_cancelled" => MemoryError::RunCancelled,
            "run_launch_failed" => MemoryError::RunLaunchFailed { message },
            "lock" => MemoryError::Lock { message },
            "not_implemented" => MemoryError::NotImplemented { feature: message },
            "unavailable" => MemoryError::Unavailable { message },
            "malformed_response" => MemoryError::MalformedResponse { message },
            _ => MemoryError::Internal { message },
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_yaml_ng::{Mapping, Value};

    use super::{ErrorEnvelope, PageWire};
    use crate::{
        error::MemoryError,
        model::{Page, PageMetadata, Revision},
        path::PagePath,
    };

    #[test]
    fn page_wire_preserves_schema_and_extra_frontmatter() {
        let mut extra = Mapping::new();
        extra.insert(
            Value::String("custom".to_owned()),
            Value::String("kept".to_owned()),
        );
        let now = Utc::now();
        let page = Page {
            path: PagePath::parse("notes/example")
                .unwrap_or_else(|error| panic!("valid path: {error}")),
            metadata: PageMetadata {
                schema_version: 7,
                title: "Example".to_owned(),
                tags: vec!["test".to_owned()],
                created_at: now,
                updated_at: now,
                created_by: Some("one".to_owned()),
                updated_by: Some("two".to_owned()),
                extra,
            },
            body: "body".to_owned(),
            revision: Revision("revision".to_owned()),
        };

        let encoded = serde_json::to_vec(&PageWire::from_page(&page, Vec::new()))
            .unwrap_or_else(|error| panic!("serialize page wire: {error}"));
        let wire: PageWire = serde_json::from_slice(&encoded)
            .unwrap_or_else(|error| panic!("deserialize page wire: {error}"));
        let round_trip =
            Page::try_from(wire).unwrap_or_else(|error| panic!("convert page wire: {error}"));
        assert_eq!(round_trip, page);
    }

    #[test]
    fn command_error_round_trip_preserves_rejected_command() {
        let error =
            ErrorEnvelope::from_error(&MemoryError::command_not_allowed("rm")).into_memory_error();
        assert!(matches!(
            error,
            MemoryError::CommandNotAllowed { command } if command == "rm"
        ));
    }
}
