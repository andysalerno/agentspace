//! The storage abstraction beneath [`crate::service::MemoryService`].
//!
//! `MemoryStore` is deliberately low-level and mechanical: it knows how to
//! read, write, rename, and enumerate `.md` files atomically and under a
//! store-wide lock, but it has no knowledge of frontmatter, tags, links, or
//! revision-conflict policy. Those domain rules live in `MemoryService`,
//! which composes several store calls inside a single [`MemoryStore::with_lock`]
//! critical section when an operation (such as a move that rewrites other
//! pages' links) must appear atomic to every other local or remote caller.

use crate::{error::MemoryError, model::Revision, path::PagePath};

/// One `.md` file discovered while scanning the store, before path
/// validation. Used by `memory check` to report files whose name is not a
/// valid page path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedFile {
    /// The `/`-joined path relative to the store root, including `.md`.
    pub relative_path: String,
    /// `Some` when `relative_path` parses as a valid [`PagePath`].
    pub parsed: Option<PagePath>,
}

/// The storage abstraction implemented by [`crate::fs_store::FilesystemMemoryStore`].
///
/// Every mutating method performs its own atomic write (temporary file plus
/// rename); callers that need several mutations to appear as one atomic
/// operation must wrap them in [`MemoryStore::with_lock`].
pub trait MemoryStore: Send + Sync {
    /// The filesystem root this store is rooted at. Not part of the
    /// agent-facing contract; used internally to resolve `memory run`'s
    /// working directory.
    fn root(&self) -> &std::path::Path;

    /// Runs `operation` while holding the store-wide exclusive lock shared
    /// by every local and remote caller. Reentrant calls from within
    /// `operation` on the same store must not attempt to lock again.
    fn with_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, MemoryError>,
    ) -> Result<T, MemoryError>;

    /// Reads a page's exact stored bytes and their revision.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::NotFound`] if no page exists at `path`.
    fn read_bytes(&self, path: &PagePath) -> Result<(Vec<u8>, Revision), MemoryError>;

    /// Returns whether a page currently exists at `path`.
    fn exists(&self, path: &PagePath) -> Result<bool, MemoryError>;

    /// Atomically writes `bytes` as the page at `path`, creating parent
    /// directories as needed.
    fn write_bytes(&self, path: &PagePath, bytes: &[u8]) -> Result<Revision, MemoryError>;

    /// Removes the page at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::NotFound`] if no page exists at `path`.
    fn remove_file(&self, path: &PagePath) -> Result<(), MemoryError>;

    /// Commits a page move and its rewritten referrers as one transaction.
    ///
    /// Implementations must stage every destination before changing any
    /// authoritative page and restore the original files if committing any
    /// staged change fails.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::NotFound`] if `source` does not exist, or
    /// [`MemoryError::AlreadyExists`] if `destination` already exists.
    fn commit_move(
        &self,
        source: &PagePath,
        destination: &PagePath,
        destination_bytes: &[u8],
        replacements: &[(PagePath, Vec<u8>)],
    ) -> Result<(), MemoryError>;

    /// Enumerates every valid page path in the store, in deterministic
    /// sorted order. Files whose name does not parse as a valid [`PagePath`]
    /// or that are (or are reached through) symlinks are silently omitted;
    /// use [`MemoryStore::scan`] to also observe those.
    fn list_pages(&self) -> Result<Vec<PagePath>, MemoryError>;

    /// Enumerates every `.md` file in the store, including ones whose name
    /// does not parse as a valid [`PagePath`], for integrity checking.
    /// Symlinked files and directories are omitted; they are never treated
    /// as pages.
    fn scan(&self) -> Result<Vec<ScannedFile>, MemoryError>;
}
