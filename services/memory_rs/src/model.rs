//! Transport-neutral domain models shared by every `MemoryClient` transport.
//!
//! These types are constructed and consumed identically whether the caller
//! is `DirectMemoryClient` in-process or a future `HttpMemoryClient` talking
//! JSON over HTTP; only serialization differs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Mapping;
use sha2::{Digest, Sha256};

use crate::path::PagePath;

/// The current on-disk frontmatter schema version written by this crate.
pub const SCHEMA_VERSION: u64 = 1;

/// A deterministic digest of a page's exact stored bytes, returned on every
/// read and required (optionally) on every mutation to detect stale writes.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Revision(pub String);

impl Revision {
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(format!("{:x}", hasher.finalize()))
    }
}

impl std::fmt::Display for Revision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Parsed YAML frontmatter for a page. Required fields are strongly typed;
/// any additional fields present in the source document are preserved
/// verbatim in `extra` so the format can evolve additively.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageMetadata {
    pub schema_version: u64,
    pub title: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    /// Unknown frontmatter fields, preserved in their original order.
    pub extra: Mapping,
}

/// A fully loaded page: its path, metadata, Markdown body, and revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Page {
    pub path: PagePath,
    pub metadata: PageMetadata,
    pub body: String,
    pub revision: Revision,
}

/// A `path -> tags/title` summary used by listing and query results, cheaper
/// to produce than a full [`Page`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageSummary {
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

/// A normalized tag with the number of pages that reference it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

/// One relative Markdown link discovered in a page body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageLink {
    /// The link text as written, e.g. `[Alice](../people/alice.md)`.
    pub text: String,
    /// The raw link target as written in the source Markdown.
    pub raw_target: String,
    /// The normalized logical page path the link resolves to, if it looks
    /// like a relative link to another store page.
    pub resolved_path: Option<String>,
    /// Whether `resolved_path` currently exists in the store.
    pub broken: bool,
}

/// One inbound reference to a page, discovered while scanning every other
/// page for a matching relative link.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Backlink {
    pub from: String,
    pub text: String,
    pub raw_target: String,
}

/// The result of `memory links <path> [--backlinks]`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinksReport {
    pub path: String,
    pub outgoing: Vec<PageLink>,
    pub backlinks: Vec<Backlink>,
}

/// A request to create or replace a page's content.
#[derive(Clone, Debug)]
pub struct WritePageRequest {
    pub path: PagePath,
    /// `None` preserves the existing title on update; required on create.
    pub title: Option<String>,
    /// `None` preserves existing tags on update (empty on create). `Some`
    /// replaces the full tag set, after normalization.
    pub tags: Option<Vec<String>>,
    pub body: String,
    /// Allows overwriting an existing page without an expected revision.
    pub overwrite: bool,
    pub expected_revision: Option<String>,
    pub actor: Option<String>,
}

/// A request to move (rename) a page, optionally guarded by a revision.
#[derive(Clone, Debug)]
pub struct MovePageRequest {
    pub source: PagePath,
    pub destination: PagePath,
    pub expected_revision: Option<String>,
    pub actor: Option<String>,
}

/// The result of a successful move, including inbound links updated in other
/// pages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MoveOutcome {
    pub source: String,
    pub destination: String,
    pub revision: String,
    pub updated_referrers: Vec<String>,
}

/// A request to delete a page, optionally guarded by a revision.
#[derive(Clone, Debug)]
pub struct RemovePageRequest {
    pub path: PagePath,
    pub expected_revision: Option<String>,
}

/// Filters shared by `pages ls` and `query`.
#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub under: Option<PagePath>,
    pub with_tags: Vec<String>,
    pub limit: Option<usize>,
}

/// A full-text query over path, title, tags, and body.
#[derive(Clone, Debug, Default)]
pub struct QueryRequest {
    pub text: String,
    pub filter: ListFilter,
}

/// One `check` finding, describing a problem with a specific page or the
/// store as a whole.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckIssue {
    pub path: Option<String>,
    pub message: String,
}

/// A full store integrity report.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckReport {
    pub issues: Vec<CheckIssue>,
}

impl CheckReport {
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}
