//! Root-relative page path parsing, validation, and filesystem resolution.
//!
//! A [`PagePath`] is the canonical identity of a memory page. It is always
//! UTF-8, always uses `/` as its logical separator regardless of transport or
//! host OS, and never contains `.`, `..`, empty segments, or control
//! characters. The `.md` suffix is optional on input and never present in the
//! logical form returned by [`PagePath::as_str`].

use std::{
    fmt::{self, Display, Formatter},
    path::{Component, Path, PathBuf},
};

use crate::error::MemoryError;

/// Maximum number of `/`-separated segments in a page path.
pub const MAX_PATH_SEGMENTS: usize = 32;
/// Maximum length, in bytes, of the logical page path (excluding `.md`).
pub const MAX_PATH_LENGTH: usize = 512;
/// Maximum length, in bytes, of a single path segment.
pub const MAX_SEGMENT_LENGTH: usize = 128;

const MARKDOWN_EXTENSION: &str = ".md";

/// A validated, root-relative identity for a Markdown memory page.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PagePath {
    segments: Vec<String>,
}

impl PagePath {
    /// Parses and validates a root-relative page path supplied by a CLI
    /// argument, query parameter, or Markdown link target.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidPath`] if the input is empty, absolute,
    /// contains `.`/`..`/empty segments, contains control characters, or
    /// exceeds the configured size limits.
    pub fn parse(input: &str) -> Result<Self, MemoryError> {
        if input.is_empty() {
            return Err(MemoryError::invalid_path(input, "path must not be empty"));
        }
        if input.len() > MAX_PATH_LENGTH {
            return Err(MemoryError::invalid_path(
                input,
                format!("path exceeds {MAX_PATH_LENGTH} bytes"),
            ));
        }
        if input.starts_with('/') {
            return Err(MemoryError::invalid_path(
                input,
                "absolute paths are not allowed",
            ));
        }
        if input.contains('\\') {
            return Err(MemoryError::invalid_path(
                input,
                "backslash is not a valid path separator",
            ));
        }

        let trimmed = input.strip_suffix(MARKDOWN_EXTENSION).unwrap_or(input);

        let mut segments = Vec::new();
        for raw_segment in trimmed.split('/') {
            if raw_segment.is_empty() {
                return Err(MemoryError::invalid_path(
                    input,
                    "path segments must not be empty",
                ));
            }
            if raw_segment == "." || raw_segment == ".." {
                return Err(MemoryError::invalid_path(
                    input,
                    "path must not contain '.' or '..' segments",
                ));
            }
            if raw_segment.len() > MAX_SEGMENT_LENGTH {
                return Err(MemoryError::invalid_path(
                    input,
                    format!("segment {raw_segment:?} exceeds {MAX_SEGMENT_LENGTH} bytes"),
                ));
            }
            if raw_segment.chars().any(char::is_control) {
                return Err(MemoryError::invalid_path(
                    input,
                    "path must not contain control characters",
                ));
            }
            segments.push(raw_segment.to_owned());
        }

        if segments.len() > MAX_PATH_SEGMENTS {
            return Err(MemoryError::invalid_path(
                input,
                format!("path exceeds {MAX_PATH_SEGMENTS} segments"),
            ));
        }

        Ok(Self { segments })
    }

    /// The logical, `.md`-free, `/`-separated form of this path.
    #[must_use]
    pub fn as_str(&self) -> String {
        self.segments.join("/")
    }

    /// The final path segment, without the `.md` suffix.
    #[must_use]
    pub fn file_stem(&self) -> &str {
        self.segments.last().map_or("", String::as_str)
    }

    /// The parent directory of this path, if any.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() <= 1 {
            return None;
        }
        Some(Self {
            segments: self.segments[..self.segments.len() - 1].to_vec(),
        })
    }

    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// The path relative to a store root, including the `.md` suffix, using
    /// the host's native path separators.
    #[must_use]
    pub fn relative_file_path(&self) -> PathBuf {
        let mut path = PathBuf::new();
        for segment in &self.segments {
            path.push(segment);
        }
        path.set_extension("md");
        path
    }

    /// Resolves this path to an absolute filesystem path beneath `root`,
    /// rejecting symlink traversal in any existing ancestor directory or in
    /// the final component itself.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::InvalidPath`] if any ancestor component, or the
    /// final component, is a symlink.
    pub fn resolve_within(&self, root: &Path) -> Result<PathBuf, MemoryError> {
        let mut current = root.to_path_buf();
        let file_name = format!("{}.md", self.file_stem());
        let mut remaining: Vec<&str> = self.segments[..self.segments.len() - 1]
            .iter()
            .map(String::as_str)
            .collect();
        remaining.push(file_name.as_str());

        for component in &remaining {
            current.push(component);
            reject_symlink(&current)?;
        }

        Ok(current)
    }
}

impl Display for PagePath {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_str())
    }
}

fn reject_symlink(path: &Path) -> Result<(), MemoryError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(MemoryError::invalid_path(
                    path.to_string_lossy(),
                    "symlinks are not allowed inside the memory store",
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(MemoryError::from(error)),
    }
}

/// Validates that a canonicalized store root is an absolute, existing
/// directory containing no path component that is itself a symlink, used
/// once at store construction.
pub fn validate_root(root: &Path) -> Result<PathBuf, MemoryError> {
    if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
        return Err(MemoryError::invalid_path(
            root.to_string_lossy(),
            "the memory store root must not be a symlink",
        ));
    }
    let canonical = std::fs::canonicalize(root)?;
    for component in canonical.components() {
        if matches!(component, Component::ParentDir) {
            return Err(MemoryError::invalid_path(
                canonical.to_string_lossy(),
                "store root must not contain '..' components",
            ));
        }
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::PagePath;

    #[test]
    fn parses_simple_path() {
        let path =
            PagePath::parse("people/alice").unwrap_or_else(|error| panic!("valid path: {error}"));
        assert_eq!(path.as_str(), "people/alice");
        assert_eq!(path.file_stem(), "alice");
    }

    #[test]
    fn strips_markdown_suffix() {
        let path = PagePath::parse("people/alice.md")
            .unwrap_or_else(|error| panic!("valid path: {error}"));
        assert_eq!(path.as_str(), "people/alice");
    }

    #[test]
    fn rejects_absolute_path() {
        let error = PagePath::parse("/etc/passwd")
            .map_or_else(|error| error, |_| panic!("must reject absolute path"));
        assert!(matches!(
            error,
            crate::error::MemoryError::InvalidPath { .. }
        ));
    }

    #[test]
    fn rejects_parent_traversal() {
        PagePath::parse("../etc/passwd")
            .map_or_else(|error| error, |_| panic!("must reject traversal"));
        PagePath::parse("people/../../etc")
            .map_or_else(|error| error, |_| panic!("must reject traversal"));
    }

    #[test]
    fn rejects_empty_segments() {
        PagePath::parse("people//alice")
            .map_or_else(|error| error, |_| panic!("must reject empty segment"));
        PagePath::parse("").map_or_else(|error| error, |_| panic!("must reject empty path"));
        PagePath::parse("people/")
            .map_or_else(|error| error, |_| panic!("must reject trailing slash"));
    }

    #[test]
    fn rejects_control_characters() {
        PagePath::parse("people/ali\u{0007}ce")
            .map_or_else(|error| error, |_| panic!("must reject control chars"));
    }

    #[test]
    fn parent_returns_none_at_root() {
        let path = PagePath::parse("alice").unwrap_or_else(|error| panic!("valid path: {error}"));
        assert!(path.parent().is_none());
    }

    #[test]
    fn parent_strips_last_segment() {
        let path = PagePath::parse("people/nested/alice")
            .unwrap_or_else(|error| panic!("valid path: {error}"));
        let parent = path.parent().unwrap_or_else(|| panic!("has parent"));
        assert_eq!(parent.as_str(), "people/nested");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_store_root() {
        let parent = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let target = parent.path().join("target");
        std::fs::create_dir(&target).unwrap_or_else(|error| panic!("create target: {error}"));
        let link = parent.path().join("link");
        std::os::unix::fs::symlink(&target, &link)
            .unwrap_or_else(|error| panic!("create symlink: {error}"));

        let error = super::validate_root(&link)
            .map_or_else(|error| error, |_| panic!("must reject symlink root"));
        assert!(matches!(
            error,
            crate::error::MemoryError::InvalidPath { .. }
        ));
    }
}
