//! [`FilesystemMemoryStore`]: the filesystem-backed [`MemoryStore`]
//! implementation, storing every page as a Markdown file beneath a root
//! directory.

use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use fs4::FileExt;

use crate::{
    error::MemoryError,
    model::Revision,
    path::{self, PagePath},
    store::{MemoryStore, ScannedFile},
};

const LOCK_FILE_NAME: &str = ".memory.lock";
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A [`MemoryStore`] backed by a directory of Markdown files, with an
/// exclusive `flock`-based lock file shared by every local and remote
/// process using the same root.
#[derive(Clone, Debug)]
pub struct FilesystemMemoryStore {
    root: PathBuf,
}

impl FilesystemMemoryStore {
    /// Opens (creating if necessary) a filesystem memory store rooted at
    /// `root`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Io`] if `root` cannot be created or
    /// canonicalized, or [`MemoryError::InvalidPath`] if it contains a
    /// disallowed component.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, MemoryError> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        let canonical = path::validate_root(root)?;
        Ok(Self { root: canonical })
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE_NAME)
    }
}

impl MemoryStore for FilesystemMemoryStore {
    fn root(&self) -> &Path {
        &self.root
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, MemoryError>,
    ) -> Result<T, MemoryError> {
        let lock_path = self.lock_path();
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        FileExt::lock(&lock_file).map_err(|error| MemoryError::lock(error.to_string()))?;
        let result = operation();
        let _unlock_result = FileExt::unlock(&lock_file);
        result
    }

    fn read_bytes(&self, path: &PagePath) -> Result<(Vec<u8>, Revision), MemoryError> {
        let full_path = path.resolve_within(&self.root)?;
        match std::fs::read(&full_path) {
            Ok(bytes) => {
                let revision = Revision::of(&bytes);
                Ok((bytes, revision))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(MemoryError::not_found(path.as_str()))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn exists(&self, path: &PagePath) -> Result<bool, MemoryError> {
        let full_path = path.resolve_within(&self.root)?;
        Ok(full_path.is_file())
    }

    fn write_bytes(&self, path: &PagePath, bytes: &[u8]) -> Result<Revision, MemoryError> {
        let full_path = path.resolve_within(&self.root)?;
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let temp_path = temp_path_for(&full_path);
        write_atomically(&temp_path, &full_path, bytes)?;
        Ok(Revision::of(bytes))
    }

    fn remove_file(&self, path: &PagePath) -> Result<(), MemoryError> {
        let full_path = path.resolve_within(&self.root)?;
        std::fs::remove_file(&full_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                MemoryError::not_found(path.as_str())
            } else {
                error.into()
            }
        })
    }

    fn commit_move(
        &self,
        source: &PagePath,
        destination: &PagePath,
        destination_bytes: &[u8],
        replacements: &[(PagePath, Vec<u8>)],
    ) -> Result<(), MemoryError> {
        let source_full = source.resolve_within(&self.root)?;
        let destination_full = destination.resolve_within(&self.root)?;

        if !source_full.is_file() {
            return Err(MemoryError::not_found(source.as_str()));
        }
        if destination_full.exists() {
            return Err(MemoryError::already_exists(destination.as_str()));
        }

        let mut unique_paths = BTreeSet::new();
        for (path, _bytes) in replacements {
            if path == source || path == destination || !unique_paths.insert(path.clone()) {
                return Err(MemoryError::invalid_path(
                    path.as_str(),
                    "move transaction contains a duplicate or reserved replacement path",
                ));
            }
            if !path.resolve_within(&self.root)?.is_file() {
                return Err(MemoryError::not_found(path.as_str()));
            }
        }

        let transaction_id = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut staged = Vec::with_capacity(replacements.len() + 1);
        stage_change(
            &destination_full,
            destination_bytes,
            transaction_id,
            &mut staged,
        )?;
        for (path, bytes) in replacements {
            let full_path = path.resolve_within(&self.root)?;
            if let Err(error) = stage_change(&full_path, bytes, transaction_id, &mut staged) {
                cleanup_staged(&staged);
                return Err(error);
            }
        }

        let mut backups = Vec::with_capacity(replacements.len() + 1);
        if let Err(error) = backup_file(&source_full, transaction_id, &mut backups) {
            cleanup_staged(&staged);
            return Err(error);
        }
        for (path, _bytes) in replacements {
            let full_path = path.resolve_within(&self.root)?;
            if let Err(error) = backup_file(&full_path, transaction_id, &mut backups) {
                restore_backups(&backups);
                cleanup_staged(&staged);
                return Err(error);
            }
        }

        let mut installed = Vec::with_capacity(staged.len());
        for (temporary, destination) in &staged {
            if let Err(error) = std::fs::rename(temporary, destination) {
                remove_installed(&installed);
                restore_backups(&backups);
                cleanup_staged(&staged);
                return Err(error.into());
            }
            installed.push(destination.clone());
        }

        for (backup, _original) in backups {
            let _cleanup_result = std::fs::remove_file(backup);
        }
        Ok(())
    }

    fn list_pages(&self) -> Result<Vec<PagePath>, MemoryError> {
        let mut pages: Vec<PagePath> = self
            .scan()?
            .into_iter()
            .filter_map(|entry| entry.parsed)
            .collect();
        pages.sort();
        Ok(pages)
    }

    fn scan(&self) -> Result<Vec<ScannedFile>, MemoryError> {
        let mut results = Vec::new();
        walk(&self.root, &self.root, &mut results)?;
        results.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(results)
    }
}

fn temp_path_for(destination: &Path) -> PathBuf {
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("page");
    let temp_name = format!(".{file_name}.tmp-{}-{counter}", std::process::id());
    destination.with_file_name(temp_name)
}

fn transaction_path_for(destination: &Path, marker: &str, transaction_id: u64) -> PathBuf {
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("page");
    destination.with_file_name(format!(
        ".{file_name}.{marker}-{}-{transaction_id}",
        std::process::id()
    ))
}

fn stage_change(
    destination: &Path,
    bytes: &[u8],
    transaction_id: u64,
    staged: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), MemoryError> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = transaction_path_for(destination, "stage", transaction_id);
    let write_result = (|| -> Result<(), MemoryError> {
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _cleanup_result = std::fs::remove_file(&temporary);
        return Err(error);
    }
    staged.push((temporary, destination.to_path_buf()));
    Ok(())
}

fn backup_file(
    original: &Path,
    transaction_id: u64,
    backups: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), MemoryError> {
    let backup = transaction_path_for(original, "backup", transaction_id);
    std::fs::rename(original, &backup)?;
    backups.push((backup, original.to_path_buf()));
    Ok(())
}

fn cleanup_staged(staged: &[(PathBuf, PathBuf)]) {
    for (temporary, _destination) in staged {
        let _cleanup_result = std::fs::remove_file(temporary);
    }
}

fn restore_backups(backups: &[(PathBuf, PathBuf)]) {
    for (backup, original) in backups.iter().rev() {
        let _restore_result = std::fs::rename(backup, original);
    }
}

fn remove_installed(installed: &[PathBuf]) {
    for path in installed.iter().rev() {
        let _remove_result = std::fs::remove_file(path);
    }
}

fn write_atomically(temp_path: &Path, destination: &Path, bytes: &[u8]) -> Result<(), MemoryError> {
    let write_result = (|| -> Result<(), MemoryError> {
        let mut file = File::create(temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(error) = write_result {
        let _cleanup_result = std::fs::remove_file(temp_path);
        return Err(error);
    }

    if let Err(error) = std::fs::rename(temp_path, destination) {
        let _cleanup_result = std::fs::remove_file(temp_path);
        return Err(error.into());
    }

    Ok(())
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<ScannedFile>) -> Result<(), MemoryError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }

        let entry_path = entry.path();
        if file_type.is_dir() {
            walk(root, &entry_path, out)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }
        if entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("md")
        {
            continue;
        }

        let Ok(relative) = entry_path.strip_prefix(root) else {
            continue;
        };
        let relative_path = relative_to_logical(relative);
        let parsed = PagePath::parse(&relative_path).ok();
        out.push(ScannedFile {
            relative_path,
            parsed,
        });
    }

    Ok(())
}

fn relative_to_logical(relative: &Path) -> String {
    relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::FilesystemMemoryStore;
    use crate::{path::PagePath, store::MemoryStore};

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let store = FilesystemMemoryStore::open(dir.path())
            .unwrap_or_else(|error| panic!("open store: {error}"));
        let path =
            PagePath::parse("people/alice").unwrap_or_else(|error| panic!("valid path: {error}"));

        let revision = store
            .write_bytes(&path, b"hello")
            .unwrap_or_else(|error| panic!("write: {error}"));
        let (bytes, read_revision) = store
            .read_bytes(&path)
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(bytes, b"hello");
        assert_eq!(revision, read_revision);
    }

    #[test]
    fn read_missing_page_is_not_found() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let store = FilesystemMemoryStore::open(dir.path())
            .unwrap_or_else(|error| panic!("open store: {error}"));
        let path = PagePath::parse("missing").unwrap_or_else(|error| panic!("valid path: {error}"));
        let error = store
            .read_bytes(&path)
            .map_or_else(|error| error, |_| panic!("must be missing"));
        assert!(matches!(error, crate::error::MemoryError::NotFound { .. }));
    }

    #[test]
    fn commit_move_moves_file_and_rejects_existing_destination() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let store = FilesystemMemoryStore::open(dir.path())
            .unwrap_or_else(|error| panic!("open store: {error}"));
        let source = PagePath::parse("a").unwrap_or_else(|error| panic!("valid path: {error}"));
        let destination =
            PagePath::parse("b").unwrap_or_else(|error| panic!("valid path: {error}"));
        store
            .write_bytes(&source, b"content")
            .unwrap_or_else(|error| panic!("write source: {error}"));

        store
            .commit_move(&source, &destination, b"content", &[])
            .unwrap_or_else(|error| panic!("move: {error}"));
        assert!(
            !store
                .exists(&source)
                .unwrap_or_else(|error| panic!("exists: {error}"))
        );
        assert!(
            store
                .exists(&destination)
                .unwrap_or_else(|error| panic!("exists: {error}"))
        );

        store
            .write_bytes(&source, b"new")
            .unwrap_or_else(|error| panic!("write source again: {error}"));
        let error = store
            .commit_move(&source, &destination, b"new", &[])
            .map_or_else(
                |error| error,
                |()| panic!("must reject existing destination"),
            );
        assert!(matches!(
            error,
            crate::error::MemoryError::AlreadyExists { .. }
        ));
    }

    #[test]
    fn list_pages_finds_nested_pages_and_skips_invalid_names() {
        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let store = FilesystemMemoryStore::open(dir.path())
            .unwrap_or_else(|error| panic!("open store: {error}"));
        let nested = PagePath::parse("a/b/c").unwrap_or_else(|error| panic!("valid path: {error}"));
        store
            .write_bytes(&nested, b"content")
            .unwrap_or_else(|error| panic!("write: {error}"));

        std::fs::create_dir_all(dir.path().join("weird"))
            .unwrap_or_else(|error| panic!("mkdir: {error}"));
        std::fs::write(dir.path().join("weird/bad\u{7}name.md"), b"x")
            .unwrap_or_else(|error| panic!("write bad file: {error}"));

        let pages = store
            .list_pages()
            .unwrap_or_else(|error| panic!("list pages: {error}"));
        assert_eq!(pages, vec![nested]);

        let scanned = store.scan().unwrap_or_else(|error| panic!("scan: {error}"));
        assert!(scanned.iter().any(|entry| entry.parsed.is_none()));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_pages_are_ignored() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let store = FilesystemMemoryStore::open(dir.path())
            .unwrap_or_else(|error| panic!("open store: {error}"));
        let real = PagePath::parse("real").unwrap_or_else(|error| panic!("valid path: {error}"));
        store
            .write_bytes(&real, b"content")
            .unwrap_or_else(|error| panic!("write: {error}"));

        symlink(dir.path().join("real.md"), dir.path().join("linked.md"))
            .unwrap_or_else(|error| panic!("symlink: {error}"));

        let pages = store
            .list_pages()
            .unwrap_or_else(|error| panic!("list pages: {error}"));
        assert_eq!(pages, vec![real]);
    }
}
