//! [`MemoryService`]: all memory domain behavior (validation, tag
//! normalization, revision/conflict policy, link maintenance, queries, and
//! integrity checks) composed on top of a [`MemoryStore`].
//!
//! `MemoryService` is the single place both `DirectMemoryClient` and a
//! future `HttpMemoryClient`/Axum adapter call into, so local and remote
//! transports share identical behavior.

use chrono::Utc;

use crate::{
    error::MemoryError,
    frontmatter, links,
    model::{
        Backlink, CheckIssue, CheckReport, LinksReport, ListFilter, MoveOutcome, MovePageRequest,
        Page, PageLink, PageMetadata, PageSummary, QueryRequest, RemovePageRequest, SCHEMA_VERSION,
        TagCount, WritePageRequest,
    },
    path::PagePath,
    store::MemoryStore,
};

/// Maximum length, in bytes, of a page title.
pub const MAX_TITLE_LENGTH: usize = 256;
/// Maximum length, in bytes, of a single tag.
pub const MAX_TAG_LENGTH: usize = 64;
/// Maximum number of tags on a single page.
pub const MAX_TAGS: usize = 64;
/// Maximum size, in bytes, of a page body.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Maximum length, in bytes, of a query's text term.
pub const MAX_QUERY_LENGTH: usize = 512;
/// Default result limit for `pages ls` and `query` when the caller does not
/// specify one.
pub const DEFAULT_LIST_LIMIT: usize = 200;

/// All memory domain behavior, generic over a concrete [`MemoryStore`]
/// implementation.
#[derive(Debug)]
pub struct MemoryService<S: MemoryStore> {
    store: S,
}

impl<S: MemoryStore> MemoryService<S> {
    pub const fn new(store: S) -> Self {
        Self { store }
    }

    #[must_use]
    pub const fn store(&self) -> &S {
        &self.store
    }

    /// Creates or replaces a page.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::AlreadyExists`] if the page exists and neither
    /// `overwrite` nor a matching `expected_revision` was supplied,
    /// [`MemoryError::Conflict`] if an `expected_revision` was supplied and
    /// does not match, or a validation error for oversize input.
    #[allow(clippy::needless_pass_by_value)]
    pub fn write_page(&self, request: WritePageRequest) -> Result<Page, MemoryError> {
        validate_body(&request.body)?;
        if let Some(title) = &request.title {
            validate_title(title)?;
        }
        if let Some(tags) = &request.tags {
            validate_tags(tags)?;
        }

        self.store.with_lock(|| {
            let existing = read_optional(&self.store, &request.path)?;

            match (&request.expected_revision, &existing) {
                (Some(expected), Some((_, actual))) if *expected == actual.0 => {}
                (Some(expected), Some((_, actual))) => {
                    return Err(MemoryError::conflict(
                        request.path.as_str(),
                        Some(expected.clone()),
                        actual.0.clone(),
                    ));
                }
                (Some(_), None) => return Err(MemoryError::not_found(request.path.as_str())),
                (None, Some(_)) if !request.overwrite => {
                    return Err(MemoryError::already_exists(request.path.as_str()));
                }
                (None, _) => {}
            }

            let now = Utc::now();
            let metadata = match &existing {
                Some((bytes, _)) => {
                    let (mut previous, _body) =
                        frontmatter::parse_document(bytes, &request.path.as_str())?;
                    if let Some(title) = &request.title {
                        previous.title.clone_from(title);
                    }
                    if let Some(tags) = &request.tags {
                        previous.tags = normalize_tags(tags);
                    }
                    previous.updated_at = now;
                    previous.updated_by = request.actor.clone().or(previous.updated_by);
                    previous
                }
                None => PageMetadata {
                    schema_version: SCHEMA_VERSION,
                    title: request.title.clone().ok_or_else(|| {
                        MemoryError::invalid_path(
                            request.path.as_str(),
                            "title is required to create a page",
                        )
                    })?,
                    tags: normalize_tags(&request.tags.clone().unwrap_or_default()),
                    created_at: now,
                    updated_at: now,
                    created_by: request.actor.clone(),
                    updated_by: request.actor.clone(),
                    extra: serde_yaml_ng::Mapping::new(),
                },
            };

            let bytes = frontmatter::render_document(&metadata, &request.body)?;
            if bytes.len() > MAX_BODY_BYTES {
                return Err(MemoryError::too_large("page document", MAX_BODY_BYTES));
            }
            let revision = self.store.write_bytes(&request.path, &bytes)?;

            Ok(Page {
                path: request.path.clone(),
                metadata,
                body: frontmatter::normalize_body(&request.body),
                revision,
            })
        })
    }

    /// Reads a page by path.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::NotFound`] if no page exists at `path`, or
    /// [`MemoryError::InvalidFrontmatter`] if its stored document is
    /// malformed.
    pub fn read_page(&self, path: &PagePath) -> Result<Page, MemoryError> {
        let (bytes, revision) = self.store.read_bytes(path)?;
        let (metadata, body) = frontmatter::parse_document(&bytes, &path.as_str())?;
        Ok(Page {
            path: path.clone(),
            metadata,
            body,
            revision,
        })
    }

    /// Moves (renames) a page, rewriting relative inline Markdown links in
    /// every other page that referenced it.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::NotFound`] if `source` does not exist,
    /// [`MemoryError::AlreadyExists`] if `destination` already exists, or
    /// [`MemoryError::Conflict`] if an `expected_revision` was supplied and
    /// does not match.
    #[allow(clippy::needless_pass_by_value)]
    pub fn move_page(&self, request: MovePageRequest) -> Result<MoveOutcome, MemoryError> {
        if request.source == request.destination {
            return Err(MemoryError::invalid_path(
                request.source.as_str(),
                "source and destination must differ",
            ));
        }

        self.store.with_lock(|| {
            let (_bytes, revision) = self.store.read_bytes(&request.source)?;
            if let Some(expected) = &request.expected_revision
                && *expected != revision.0
            {
                return Err(MemoryError::conflict(
                    request.source.as_str(),
                    Some(expected.clone()),
                    revision.0,
                ));
            }
            if self.store.exists(&request.destination)? {
                return Err(MemoryError::already_exists(request.destination.as_str()));
            }

            let mut updated_referrers = Vec::new();
            let mut replacements = Vec::new();
            for page_path in self.store.list_pages()? {
                if page_path == request.source {
                    continue;
                }
                let (page_bytes, _revision) = self.store.read_bytes(&page_path)?;
                let Ok((metadata, body)) =
                    frontmatter::parse_document(&page_bytes, &page_path.as_str())
                else {
                    continue;
                };
                let (new_body, changed) =
                    links::rewrite_links(&page_path, &body, &request.source, &request.destination);
                if changed {
                    let new_bytes = frontmatter::render_document(&metadata, &new_body)?;
                    updated_referrers.push(page_path.as_str());
                    replacements.push((page_path, new_bytes));
                }
            }
            let (source_bytes, _source_revision) = self.store.read_bytes(&request.source)?;
            self.store.commit_move(
                &request.source,
                &request.destination,
                &source_bytes,
                &replacements,
            )?;

            Ok(MoveOutcome {
                source: request.source.as_str(),
                destination: request.destination.as_str(),
                revision: revision.0,
                updated_referrers,
            })
        })
    }

    /// Deletes a page.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::NotFound`] if no page exists at `path`, or
    /// [`MemoryError::Conflict`] if an `expected_revision` was supplied and
    /// does not match.
    #[allow(clippy::needless_pass_by_value)]
    pub fn remove_page(&self, request: RemovePageRequest) -> Result<(), MemoryError> {
        self.store.with_lock(|| {
            let (_bytes, revision) = self.store.read_bytes(&request.path)?;
            if let Some(expected) = &request.expected_revision
                && *expected != revision.0
            {
                return Err(MemoryError::conflict(
                    request.path.as_str(),
                    Some(expected.clone()),
                    revision.0,
                ));
            }
            self.store.remove_file(&request.path)
        })
    }

    /// Lists page summaries matching `filter`, sorted by path.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be scanned.
    pub fn list_pages(&self, filter: &ListFilter) -> Result<Vec<PageSummary>, MemoryError> {
        self.query_pages(&QueryRequest {
            text: String::new(),
            filter: filter.clone(),
        })
    }

    /// Runs a case-insensitive literal text query over path, title, tags,
    /// and body, combined with `filter`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::TooLarge`] if the query text is too long, or
    /// an error if the store cannot be scanned.
    pub fn query_pages(&self, request: &QueryRequest) -> Result<Vec<PageSummary>, MemoryError> {
        if request.text.len() > MAX_QUERY_LENGTH {
            return Err(MemoryError::too_large("query text", MAX_QUERY_LENGTH));
        }
        let needle = request.text.to_lowercase();
        let with_tags: Vec<String> = request
            .filter
            .with_tags
            .iter()
            .map(|tag| normalize_tag(tag))
            .collect();
        let limit = request.filter.limit.unwrap_or(DEFAULT_LIST_LIMIT);

        let mut results = Vec::new();
        for page_path in self.store.list_pages()? {
            if let Some(under) = &request.filter.under
                && !is_under(under, &page_path)
            {
                continue;
            }

            let (bytes, _revision) = self.store.read_bytes(&page_path)?;
            let Ok((metadata, body)) = frontmatter::parse_document(&bytes, &page_path.as_str())
            else {
                continue;
            };

            if !with_tags.is_empty() && !with_tags.iter().all(|tag| metadata.tags.contains(tag)) {
                continue;
            }

            if !needle.is_empty() {
                let haystack = format!(
                    "{} {} {} {}",
                    page_path.as_str(),
                    metadata.title,
                    metadata.tags.join(" "),
                    body
                )
                .to_lowercase();
                if !haystack.contains(&needle) {
                    continue;
                }
            }

            results.push(PageSummary {
                path: page_path.as_str(),
                title: metadata.title,
                tags: metadata.tags,
                updated_at: metadata.updated_at,
            });
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    /// Lists every normalized tag currently in use, with page counts, sorted
    /// alphabetically.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be scanned.
    pub fn list_tags(&self) -> Result<Vec<TagCount>, MemoryError> {
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for page_path in self.store.list_pages()? {
            let (bytes, _revision) = self.store.read_bytes(&page_path)?;
            let Ok((metadata, _body)) = frontmatter::parse_document(&bytes, &page_path.as_str())
            else {
                continue;
            };
            for tag in metadata.tags {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }
        Ok(counts
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect())
    }

    /// Reports outgoing links for `path`, and inbound backlinks from every
    /// other page when `include_backlinks` is set.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::NotFound`] if no page exists at `path`.
    pub fn links(
        &self,
        path: &PagePath,
        include_backlinks: bool,
    ) -> Result<LinksReport, MemoryError> {
        let page = self.read_page(path)?;
        let parsed = links::parse_links(path, &page.body);
        let mut outgoing = Vec::with_capacity(parsed.len());
        for link in parsed {
            let broken = match &link.resolved {
                Some(resolved) => !self.store.exists(resolved)?,
                None => false,
            };
            outgoing.push(PageLink {
                text: link.text,
                raw_target: link.raw_target,
                resolved_path: link.resolved.map(|resolved| resolved.as_str()),
                broken,
            });
        }

        let mut backlinks = Vec::new();
        if include_backlinks {
            for other_path in self.store.list_pages()? {
                if &other_path == path {
                    continue;
                }
                let (bytes, _revision) = self.store.read_bytes(&other_path)?;
                let Ok((_metadata, body)) =
                    frontmatter::parse_document(&bytes, &other_path.as_str())
                else {
                    continue;
                };
                for link in links::parse_links(&other_path, &body) {
                    if link.resolved.as_ref() == Some(path) {
                        backlinks.push(Backlink {
                            from: other_path.as_str(),
                            text: link.text,
                            raw_target: link.raw_target,
                        });
                    }
                }
            }
        }

        Ok(LinksReport {
            path: path.as_str(),
            outgoing,
            backlinks,
        })
    }

    /// Scans every file in the store, reporting invalid page paths,
    /// malformed frontmatter, duplicate normalized tags, and broken internal
    /// links.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be scanned.
    pub fn check(&self) -> Result<CheckReport, MemoryError> {
        let mut issues = Vec::new();
        let scanned = self.store.scan()?;

        for entry in &scanned {
            if entry.parsed.is_none() {
                issues.push(CheckIssue {
                    path: Some(entry.relative_path.clone()),
                    message: "file name is not a valid page path".to_owned(),
                });
            }
        }

        let valid_paths: std::collections::BTreeSet<PagePath> = scanned
            .iter()
            .filter_map(|entry| entry.parsed.clone())
            .collect();

        for page_path in &valid_paths {
            let (bytes, _revision) = match self.store.read_bytes(page_path) {
                Ok(value) => value,
                Err(error) => {
                    issues.push(CheckIssue {
                        path: Some(page_path.as_str()),
                        message: error.to_string(),
                    });
                    continue;
                }
            };

            let (metadata, body) = match frontmatter::parse_document(&bytes, &page_path.as_str()) {
                Ok(value) => value,
                Err(error) => {
                    issues.push(CheckIssue {
                        path: Some(page_path.as_str()),
                        message: error.to_string(),
                    });
                    continue;
                }
            };

            let mut seen_tags = std::collections::BTreeSet::new();
            for tag in &metadata.tags {
                if !seen_tags.insert(tag.clone()) {
                    issues.push(CheckIssue {
                        path: Some(page_path.as_str()),
                        message: format!("duplicate tag {tag:?}"),
                    });
                }
            }

            for link in links::parse_links(page_path, &body) {
                if let Some(resolved) = &link.resolved
                    && !valid_paths.contains(resolved)
                {
                    issues.push(CheckIssue {
                        path: Some(page_path.as_str()),
                        message: format!("broken link to {:?}", resolved.as_str()),
                    });
                }
            }
        }

        Ok(CheckReport { issues })
    }
}

fn read_optional<S: MemoryStore>(
    store: &S,
    path: &PagePath,
) -> Result<Option<(Vec<u8>, crate::model::Revision)>, MemoryError> {
    match store.read_bytes(path) {
        Ok(value) => Ok(Some(value)),
        Err(MemoryError::NotFound { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_under(prefix: &PagePath, candidate: &PagePath) -> bool {
    let prefix_segments = prefix.segments();
    let candidate_segments = candidate.segments();
    candidate_segments.len() >= prefix_segments.len()
        && candidate_segments[..prefix_segments.len()] == *prefix_segments
}

fn validate_title(title: &str) -> Result<(), MemoryError> {
    if title.is_empty() {
        return Err(MemoryError::invalid_path(
            "title",
            "title must not be empty",
        ));
    }
    if title.len() > MAX_TITLE_LENGTH {
        return Err(MemoryError::too_large("title", MAX_TITLE_LENGTH));
    }
    Ok(())
}

fn validate_tags(tags: &[String]) -> Result<(), MemoryError> {
    if tags.len() > MAX_TAGS {
        return Err(MemoryError::too_large("tags", MAX_TAGS));
    }
    for tag in tags {
        if tag.trim().is_empty() {
            return Err(MemoryError::invalid_path("tag", "tags must not be empty"));
        }
        if tag.len() > MAX_TAG_LENGTH {
            return Err(MemoryError::too_large("tag", MAX_TAG_LENGTH));
        }
    }
    Ok(())
}

const fn validate_body(body: &str) -> Result<(), MemoryError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(MemoryError::too_large("page body", MAX_BODY_BYTES));
    }
    Ok(())
}

fn normalize_tag(tag: &str) -> String {
    tag.trim().to_lowercase()
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = tags.iter().map(|tag| normalize_tag(tag)).collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::MemoryService;
    use crate::{
        fs_store::FilesystemMemoryStore,
        model::{ListFilter, MovePageRequest, QueryRequest, RemovePageRequest, WritePageRequest},
        path::PagePath,
    };

    fn service(root: &std::path::Path) -> MemoryService<FilesystemMemoryStore> {
        let store =
            FilesystemMemoryStore::open(root).unwrap_or_else(|error| panic!("open store: {error}"));
        MemoryService::new(store)
    }

    fn write_request(path: &str, title: &str, tags: &[&str], body: &str) -> WritePageRequest {
        WritePageRequest {
            path: PagePath::parse(path).unwrap_or_else(|error| panic!("valid path: {error}")),
            title: Some(title.to_owned()),
            tags: Some(tags.iter().map(|tag| (*tag).to_owned()).collect()),
            body: body.to_owned(),
            overwrite: false,
            expected_revision: None,
            actor: Some("test-agent".to_owned()),
        }
    }

    #[test]
    fn write_then_read_preserves_metadata() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        let page = service
            .write_page(write_request(
                "people/alice",
                "Alice",
                &["Birthday", "birthday"],
                "hello",
            ))
            .unwrap_or_else(|error| panic!("write: {error}"));
        assert_eq!(page.metadata.tags, vec!["birthday".to_owned()]);

        let read = service
            .read_page(
                &PagePath::parse("people/alice")
                    .unwrap_or_else(|error| panic!("valid path: {error}")),
            )
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(read.metadata.title, "Alice");
        assert_eq!(read.body, "hello\n");
    }

    #[test]
    fn write_without_overwrite_rejects_existing_page() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request("a", "A", &[], "one"))
            .unwrap_or_else(|error| panic!("first write: {error}"));
        let error = service
            .write_page(write_request("a", "A", &[], "two"))
            .map_or_else(|error| error, |_| panic!("must reject overwrite"));
        assert!(matches!(
            error,
            crate::error::MemoryError::AlreadyExists { .. }
        ));
    }

    #[test]
    fn write_with_stale_expected_revision_conflicts() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request("a", "A", &[], "one"))
            .unwrap_or_else(|error| panic!("first write: {error}"));

        let mut request = write_request("a", "A", &[], "two");
        request.expected_revision = Some("deadbeef".to_owned());
        let error = service
            .write_page(request)
            .map_or_else(|error| error, |_| panic!("must conflict"));
        assert!(matches!(error, crate::error::MemoryError::Conflict { .. }));
    }

    #[test]
    fn write_with_matching_expected_revision_succeeds() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        let first = service
            .write_page(write_request("a", "A", &[], "one"))
            .unwrap_or_else(|error| panic!("first write: {error}"));

        let mut request = write_request("a", "A", &[], "two");
        request.expected_revision = Some(first.revision.0);
        let second = service
            .write_page(request)
            .unwrap_or_else(|error| panic!("second write: {error}"));
        assert_eq!(second.body, "two\n");
    }

    #[test]
    fn query_matches_body_title_and_tags() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request(
                "people/alice",
                "Alice",
                &["friend"],
                "loves cake",
            ))
            .unwrap_or_else(|error| panic!("write: {error}"));
        service
            .write_page(write_request(
                "people/bob",
                "Bob",
                &["coworker"],
                "likes tea",
            ))
            .unwrap_or_else(|error| panic!("write: {error}"));

        let results = service
            .query_pages(&QueryRequest {
                text: "cake".to_owned(),
                filter: ListFilter::default(),
            })
            .unwrap_or_else(|error| panic!("query: {error}"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "people/alice");
    }

    #[test]
    fn list_pages_under_prefix_and_tag_filter() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request("people/alice", "Alice", &["friend"], "x"))
            .unwrap_or_else(|error| panic!("write: {error}"));
        service
            .write_page(write_request(
                "projects/agentspace",
                "AgentSpace",
                &["work"],
                "x",
            ))
            .unwrap_or_else(|error| panic!("write: {error}"));

        let filter = ListFilter {
            under: Some(
                PagePath::parse("people").unwrap_or_else(|error| panic!("valid path: {error}")),
            ),
            with_tags: vec![],
            limit: None,
        };
        let results = service
            .list_pages(&filter)
            .unwrap_or_else(|error| panic!("list: {error}"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "people/alice");

        let tag_filter = ListFilter {
            under: None,
            with_tags: vec!["work".to_owned()],
            limit: None,
        };
        let tag_results = service
            .list_pages(&tag_filter)
            .unwrap_or_else(|error| panic!("list: {error}"));
        assert_eq!(tag_results.len(), 1);
        assert_eq!(tag_results[0].path, "projects/agentspace");
    }

    #[test]
    fn move_page_updates_referrer_links() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request(
                "projects/agentspace",
                "AgentSpace",
                &[],
                "the project",
            ))
            .unwrap_or_else(|error| panic!("write target: {error}"));
        service
            .write_page(write_request(
                "people/alice",
                "Alice",
                &[],
                "Related: [AgentSpace](../projects/agentspace.md)",
            ))
            .unwrap_or_else(|error| panic!("write referrer: {error}"));

        let outcome = service
            .move_page(MovePageRequest {
                source: PagePath::parse("projects/agentspace")
                    .unwrap_or_else(|error| panic!("valid path: {error}")),
                destination: PagePath::parse("projects/renamed")
                    .unwrap_or_else(|error| panic!("valid path: {error}")),
                expected_revision: None,
                actor: None,
            })
            .unwrap_or_else(|error| panic!("move: {error}"));
        assert_eq!(outcome.updated_referrers, vec!["people/alice".to_owned()]);

        let referrer = service
            .read_page(
                &PagePath::parse("people/alice")
                    .unwrap_or_else(|error| panic!("valid path: {error}")),
            )
            .unwrap_or_else(|error| panic!("read referrer: {error}"));
        assert!(referrer.body.contains("../projects/renamed.md"));
    }

    #[test]
    fn remove_page_requires_matching_revision_when_supplied() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request("a", "A", &[], "one"))
            .unwrap_or_else(|error| panic!("write: {error}"));

        let error = service
            .remove_page(RemovePageRequest {
                path: PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}")),
                expected_revision: Some("deadbeef".to_owned()),
            })
            .map_or_else(|error| error, |()| panic!("must conflict"));
        assert!(matches!(error, crate::error::MemoryError::Conflict { .. }));

        service
            .remove_page(RemovePageRequest {
                path: PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}")),
                expected_revision: None,
            })
            .unwrap_or_else(|error| panic!("remove: {error}"));
        let error = service
            .read_page(&PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}")))
            .map_or_else(|error| error, |_| panic!("must be gone"));
        assert!(matches!(error, crate::error::MemoryError::NotFound { .. }));
    }

    #[test]
    fn check_reports_broken_links_and_duplicate_tags() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request(
                "people/alice",
                "Alice",
                &[],
                "See [Missing](../missing.md)",
            ))
            .unwrap_or_else(|error| panic!("write: {error}"));

        let report = service
            .check()
            .unwrap_or_else(|error| panic!("check: {error}"));
        assert!(!report.is_clean());
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.message.contains("broken link"))
        );
    }

    #[test]
    fn list_tags_counts_pages_per_tag() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request("a", "A", &["shared", "only-a"], "x"))
            .unwrap_or_else(|error| panic!("write: {error}"));
        service
            .write_page(write_request("b", "B", &["shared"], "x"))
            .unwrap_or_else(|error| panic!("write: {error}"));

        let tags = service
            .list_tags()
            .unwrap_or_else(|error| panic!("tags: {error}"));
        let shared = tags
            .iter()
            .find(|tag| tag.tag == "shared")
            .unwrap_or_else(|| panic!("shared tag"));
        assert_eq!(shared.count, 2);
    }

    #[test]
    fn links_reports_backlinks() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let service = service(dir.path());
        service
            .write_page(write_request("projects/agentspace", "AgentSpace", &[], "x"))
            .unwrap_or_else(|error| panic!("write target: {error}"));
        service
            .write_page(write_request(
                "people/alice",
                "Alice",
                &[],
                "[AgentSpace](../projects/agentspace.md)",
            ))
            .unwrap_or_else(|error| panic!("write referrer: {error}"));

        let report = service
            .links(
                &PagePath::parse("projects/agentspace")
                    .unwrap_or_else(|error| panic!("valid path: {error}")),
                true,
            )
            .unwrap_or_else(|error| panic!("links: {error}"));
        assert_eq!(report.backlinks.len(), 1);
        assert_eq!(report.backlinks[0].from, "people/alice");
    }

    #[test]
    fn reopening_store_reconstructs_listings() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        {
            let service = service(dir.path());
            service
                .write_page(write_request("people/alice", "Alice", &["friend"], "x"))
                .unwrap_or_else(|error| panic!("write: {error}"));
        }

        let reopened = service(dir.path());
        let results = reopened
            .list_pages(&ListFilter::default())
            .unwrap_or_else(|error| panic!("list: {error}"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "people/alice");
        let tags = reopened
            .list_tags()
            .unwrap_or_else(|error| panic!("tags: {error}"));
        assert_eq!(tags.len(), 1);
    }
}
