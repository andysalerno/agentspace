//! Deterministic config-set bundle (zip) support.
//!
//! A bundle is a zip archive whose entries are relative POSIX paths.
//! Configuration documents are the `*.yaml`/`*.yml` entries at the bundle root
//! or anywhere under a top-level `config/` directory. Every such manifest is
//! parsed and merged into a single [`ConfigDocument`]. A skill declared with
//! `spec.source.path` expands the referenced directory into an inline file map,
//! or, when the path names a single file, into a one-file skill keyed by that
//! file's basename; the path is resolved relative to the directory of the
//! manifest that declares it. YAML files that live inside a skill source
//! directory (including under `config/`) are treated as skill content, never as
//! configuration manifests. The exact uploaded bytes are retained for
//! byte-identical source export; canonical export always inlines the resolved
//! files.
//!
//! The reader enforces strict safety limits: no path traversal (including
//! backslash/absolute/Windows-drive forms), no symlinks, no duplicate entries,
//! and bounded entry count and actual-decompressed entry/total sizes.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};

use crate::config::{document::ConfigDocument, error::ConfigError, loader::DocumentAccumulator};

/// The conventional root manifest name used by the CLI and test fixtures.
///
/// Any root-level or `config/`-rooted `*.yaml`/`*.yml` entry is a configuration
/// document; this name is not special-cased by the loader.
pub const BUNDLE_MANIFEST_NAME: &str = "agentspace-config.yaml";

/// Directory prefix (besides the bundle root) under which manifests may live.
const MANIFEST_DIR_PREFIX: &str = "config/";

/// Maximum number of entries permitted in a bundle.
const MAX_ENTRIES: usize = 4096;
/// Maximum total uncompressed size permitted in a bundle (32 MiB).
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
/// Maximum uncompressed size for a single entry (8 MiB).
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
/// Chunk size used to bound decompression while reading each entry.
const READ_CHUNK_BYTES: usize = 64 * 1024;
/// Unix mode file-type mask.
const UNIX_TYPE_MASK: u32 = 0o17_0000;
/// Unix mode symlink file-type value.
const UNIX_TYPE_SYMLINK: u32 = 0o12_0000;

fn bundle_error(detail: impl Into<String>) -> ConfigError {
    ConfigError::Bundle {
        detail: detail.into(),
    }
}

/// Reject entry names that could traverse outside the archive root: absolute
/// POSIX paths, Windows backslash separators, and Windows drive-absolute forms.
fn reject_unsafe_name(name: &str) -> Result<(), ConfigError> {
    if name.contains('\\') {
        return Err(bundle_error(format!(
            "bundle entry {name:?} contains a backslash path separator, which is not allowed"
        )));
    }
    if name.starts_with('/') {
        return Err(bundle_error(format!(
            "bundle entry {name:?} is an absolute path, which is not allowed"
        )));
    }
    // Windows drive-absolute form such as "C:foo" or "C:/foo".
    if let Some((prefix, _)) = name.split_once(':')
        && prefix.len() == 1
        && prefix.chars().all(|c| c.is_ascii_alphabetic())
    {
        return Err(bundle_error(format!(
            "bundle entry {name:?} uses a Windows drive path, which is not allowed"
        )));
    }
    Ok(())
}

/// Read the decompressed content of a single entry while enforcing the
/// per-entry and running-total decompressed-byte limits. Limits are checked
/// against the actual bytes produced, never the archive's declared size.
fn read_entry_bounded(
    entry: &mut zip::read::ZipFile<'_>,
    display: &str,
    total: &mut u64,
) -> Result<Vec<u8>, ConfigError> {
    let mut contents: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK_BYTES];
    loop {
        let read = entry
            .read(&mut chunk)
            .map_err(|error| bundle_error(format!("failed to read entry {display}: {error}")))?;
        if read == 0 {
            break;
        }
        if contents.len() as u64 + read as u64 > MAX_ENTRY_BYTES {
            return Err(bundle_error(format!(
                "bundle entry {display} exceeds the per-file limit of {MAX_ENTRY_BYTES} \
                 decompressed bytes"
            )));
        }
        *total = total.saturating_add(read as u64);
        if *total > MAX_TOTAL_BYTES {
            return Err(bundle_error(format!(
                "bundle exceeds the total limit of {MAX_TOTAL_BYTES} decompressed bytes"
            )));
        }
        contents.extend_from_slice(&chunk[..read]);
    }
    Ok(contents)
}

/// Read and validate all regular-file entries of a zip bundle into a map of
/// normalized relative path to UTF-8 content.
fn read_entries(bytes: &[u8]) -> Result<BTreeMap<String, String>, ConfigError> {
    let cursor = Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|error| bundle_error(error.to_string()))?;
    if archive.len() > MAX_ENTRIES {
        return Err(bundle_error(format!(
            "bundle has {} entries, exceeding the limit of {MAX_ENTRIES}",
            archive.len()
        )));
    }

    let mut files: BTreeMap<String, String> = BTreeMap::new();
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| bundle_error(error.to_string()))?;

        reject_unsafe_name(entry.name())?;

        if let Some(mode) = entry.unix_mode()
            && (mode & UNIX_TYPE_MASK) == UNIX_TYPE_SYMLINK
        {
            return Err(bundle_error(format!(
                "bundle entry {:?} is a symlink, which is not allowed",
                entry.name()
            )));
        }

        let Some(path) = entry.enclosed_name() else {
            return Err(bundle_error(format!(
                "bundle entry {:?} escapes the archive root",
                entry.name()
            )));
        };
        if entry.is_dir() {
            continue;
        }
        let raw_name = path
            .to_str()
            .ok_or_else(|| {
                bundle_error(format!(
                    "bundle entry {} is not valid UTF-8",
                    path.display()
                ))
            })?
            .replace('\\', "/");
        let normalized = normalize_entry_path(&raw_name)?;

        let raw = read_entry_bounded(&mut entry, &normalized, &mut total)?;
        let contents = String::from_utf8(raw).map_err(|error| {
            bundle_error(format!("entry {normalized} is not valid UTF-8: {error}"))
        })?;

        if files.insert(normalized.clone(), contents).is_some() {
            return Err(bundle_error(format!(
                "bundle contains duplicate entry {normalized:?}"
            )));
        }
    }
    Ok(files)
}

/// Canonicalize a POSIX-style entry path into a duplicate-detection key by
/// dropping empty and `.` segments and rejecting `..`. Path variants such as
/// `a/b`, `a/./b`, and `a//b` all normalize to the same key so they are treated
/// as duplicates rather than distinct files.
fn normalize_entry_path(name: &str) -> Result<String, ConfigError> {
    let mut segments: Vec<&str> = Vec::new();
    for segment in name.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                return Err(bundle_error(format!(
                    "bundle entry {name:?} escapes the archive root"
                )));
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return Err(bundle_error(format!(
            "bundle entry {name:?} does not name a file"
        )));
    }
    Ok(segments.join("/"))
}

/// Return whether an entry path names a YAML document by extension.
fn is_yaml_path(name: &str) -> bool {
    let extension = name.rsplit('/').next().unwrap_or(name).rsplit('.').next();
    matches!(extension, Some(ext) if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
}

/// Return whether a path is eligible to be a configuration manifest: a
/// root-level YAML entry or a YAML entry under a top-level `config/` directory.
fn is_candidate_manifest(path: &str) -> bool {
    is_yaml_path(path) && (!path.contains('/') || path.starts_with(MANIFEST_DIR_PREFIX))
}

/// Return the directory portion of a normalized POSIX path (empty for a
/// root-level entry).
fn parent_dir(path: &str) -> &str {
    path.rfind('/').map_or("", |index| &path[..index])
}

/// Return whether `path` lies within the directory `dir` (or equals it). An
/// empty `dir` (the bundle root) never excludes manifests.
fn is_within(path: &str, dir: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    path == dir || path.starts_with(&format!("{dir}/"))
}

/// Parse a config-set bundle into a strict [`ConfigDocument`].
///
/// Manifests are the `*.yaml`/`*.yml` entries at the bundle root or under a
/// top-level `config/` directory; each declared skill's `source.path` is
/// expanded relative to the declaring manifest's directory. YAML files inside a
/// skill source directory are treated as skill content, not manifests.
///
/// # Errors
/// Returns a [`ConfigError`] on any archive, safety, or schema violation.
pub fn load_bundle(bytes: &[u8]) -> Result<ConfigDocument, ConfigError> {
    let files = read_entries(bytes)?;
    let candidates: BTreeSet<String> = files
        .keys()
        .filter(|name| is_candidate_manifest(name))
        .cloned()
        .collect();
    if candidates.is_empty() {
        return Err(bundle_error(
            "bundle contains no configuration manifests; place *.yaml/*.yml documents at the \
             bundle root or under a top-level config/ directory",
        ));
    }

    // Pass 1: discover every skill source directory so manifests that fall
    // inside a source directory can be excluded (treated as skill content).
    let source_dirs = discover_source_dirs(&files, &candidates)?;
    let manifest_paths: BTreeSet<String> = candidates
        .iter()
        .filter(|manifest| !source_dirs.iter().any(|dir| is_within(manifest, dir)))
        .cloned()
        .collect();
    if manifest_paths.is_empty() {
        return Err(bundle_error(
            "every candidate manifest lies inside a skill source directory; no configuration \
             documents remain",
        ));
    }

    // Pass 2: merge the final manifest set, expanding skill sources for real.
    let mut accumulator = DocumentAccumulator::default();
    for manifest_path in &manifest_paths {
        let Some(source) = files.get(manifest_path) else {
            continue;
        };
        let base_dir = parent_dir(manifest_path).to_owned();
        let resolver = |path: &str| expand_source(&files, &manifest_paths, &base_dir, path);
        accumulator.merge_source(source, &resolver)?;
    }
    accumulator.finish()
}

/// Discover every skill source directory declared by the bundle's manifests so
/// that companion files (including `*.yaml`) living inside a source directory
/// are treated as skill content, not configuration manifests.
///
/// This is tolerant by design: a candidate YAML that lies inside a source
/// directory is skill content and may not parse as a manifest, so parse errors
/// are deferred while the source-directory set is grown to a fixpoint. Only
/// after the fixpoint is reached does a candidate that is still outside every
/// source directory have to parse as a valid manifest.
fn discover_source_dirs(
    files: &BTreeMap<String, String>,
    candidates: &BTreeSet<String>,
) -> Result<BTreeSet<String>, ConfigError> {
    let mut source_dirs: BTreeSet<String> = BTreeSet::new();
    // Grow the source-directory set to a fixpoint. Only manifests that are not
    // (yet) inside a known source directory contribute new source directories,
    // and parse failures are ignored here because such a candidate is most
    // likely skill content that will be excluded on a later iteration.
    loop {
        let mut added = false;
        for manifest_path in candidates {
            if source_dirs.iter().any(|dir| is_within(manifest_path, dir)) {
                continue;
            }
            let Some(source) = files.get(manifest_path) else {
                continue;
            };
            let base_dir = parent_dir(manifest_path);
            if let Ok(dirs) = declared_source_dirs(source, base_dir) {
                for dir in dirs {
                    if source_dirs.insert(dir) {
                        added = true;
                    }
                }
            }
        }
        if !added {
            break;
        }
    }
    // Every candidate that remains outside all source directories must be a
    // genuine, well-formed manifest; propagate its parse error now.
    for manifest_path in candidates {
        if source_dirs.iter().any(|dir| is_within(manifest_path, dir)) {
            continue;
        }
        let Some(source) = files.get(manifest_path) else {
            continue;
        };
        let base_dir = parent_dir(manifest_path);
        declared_source_dirs(source, base_dir)?;
    }
    Ok(source_dirs)
}

/// Parse a single manifest source only to collect the skill source directories
/// (or exact source files) it declares, resolved relative to `base_dir`. Skill
/// content is not materialized.
fn declared_source_dirs(source: &str, base_dir: &str) -> Result<BTreeSet<String>, ConfigError> {
    let dirs: RefCell<BTreeSet<String>> = RefCell::new(BTreeSet::new());
    let mut accumulator = DocumentAccumulator::default();
    let resolver = |path: &str| {
        let resolved = resolve_relative(base_dir, path)?;
        dirs.borrow_mut().insert(resolved);
        Ok(BTreeMap::new())
    };
    accumulator.merge_source(source, &resolver)?;
    Ok(dirs.into_inner())
}

/// Resolve a skill `source.path` against the declaring manifest's directory,
/// normalizing `.`/`..` segments and rejecting absolute paths or any path that
/// escapes the bundle root.
fn resolve_relative(base_dir: &str, path: &str) -> Result<String, ConfigError> {
    if path.starts_with('/') {
        return Err(bundle_error(format!(
            "skill source path {path:?} must be relative"
        )));
    }
    let combined = if base_dir.is_empty() {
        path.to_owned()
    } else {
        format!("{base_dir}/{path}")
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in combined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(bundle_error(format!(
                        "skill source path {path:?} escapes the bundle root"
                    )));
                }
            }
            other => segments.push(other),
        }
    }
    Ok(segments.join("/"))
}

/// Expand a skill `source.path` directory into an inline file map keyed by the
/// path relative to the resolved source directory. Configuration manifests are
/// never included as skill content.
fn expand_source(
    files: &BTreeMap<String, String>,
    manifest_paths: &BTreeSet<String>,
    base_dir: &str,
    path: &str,
) -> Result<BTreeMap<String, String>, ConfigError> {
    let resolved = resolve_relative(base_dir, path)?;
    // Direct-file source: `source.path` names an exact file entry (for example
    // `skills/my-skill/SKILL.md`). The resulting skill has a single file keyed
    // by that file's basename.
    if let Some(contents) = files.get(&resolved) {
        let basename = resolved.rsplit('/').next().unwrap_or(resolved.as_str());
        let mut collected = BTreeMap::new();
        collected.insert(basename.to_owned(), contents.clone());
        return Ok(collected);
    }
    let prefix = if resolved.is_empty() {
        String::new()
    } else {
        format!("{resolved}/")
    };
    let mut collected: BTreeMap<String, String> = BTreeMap::new();
    for (name, contents) in files {
        if manifest_paths.contains(name) {
            continue;
        }
        let relative = if prefix.is_empty() {
            name.as_str()
        } else if let Some(stripped) = name.strip_prefix(&prefix) {
            stripped
        } else {
            continue;
        };
        if relative.is_empty() {
            continue;
        }
        collected.insert(relative.to_owned(), contents.clone());
    }
    if collected.is_empty() {
        return Err(bundle_error(format!(
            "skill source path {path:?} (resolved to {resolved:?}) matched no files in the bundle"
        )));
    }
    Ok(collected)
}

#[cfg(test)]
mod tests {
    use std::{error::Error, io::Write};

    use zip::write::SimpleFileOptions;

    use super::{BUNDLE_MANIFEST_NAME, load_bundle};

    type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

    fn build_zip(entries: &[(&str, &str)]) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, contents) in entries {
                writer.start_file(*name, options)?;
                writer.write_all(contents.as_bytes())?;
            }
            writer.finish()?;
        }
        Ok(buffer)
    }

    const MANIFEST: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: skills/my-skill
";

    #[test]
    fn load_bundle_expands_source_directories() -> TestResult {
        let bytes = build_zip(&[
            (BUNDLE_MANIFEST_NAME, MANIFEST),
            ("skills/my-skill/SKILL.md", "# hello"),
            ("skills/my-skill/scripts/run.sh", "echo hi"),
        ])?;
        let document = load_bundle(&bytes)?;
        let skill = document
            .spec
            .skills
            .first()
            .ok_or("expected one skill in the bundle document")?;
        assert_eq!(skill.id, "my-skill");
        assert_eq!(
            skill.files.get("SKILL.md").map(String::as_str),
            Some("# hello")
        );
        assert_eq!(
            skill.files.get("scripts/run.sh").map(String::as_str),
            Some("echo hi")
        );
        Ok(())
    }

    #[test]
    fn load_bundle_requires_at_least_one_yaml_document() -> TestResult {
        let bytes = build_zip(&[("skills/my-skill/SKILL.md", "# hi")])?;
        let error = load_bundle(&bytes)
            .err()
            .ok_or("expected a rejection when no YAML documents are present")?;
        assert!(
            error.to_string().contains("no configuration manifests"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn load_bundle_rejects_duplicate_entries() -> TestResult {
        // Path variants that resolve to the same logical file must be rejected as
        // duplicates even though the archive stores them as distinct entries.
        let bytes = build_zip(&[
            (BUNDLE_MANIFEST_NAME, MANIFEST),
            ("skills/my-skill/SKILL.md", "# first"),
            ("skills/./my-skill/SKILL.md", "# second"),
        ])?;
        let error = load_bundle(&bytes)
            .err()
            .ok_or("expected a duplicate-entry rejection")?;
        assert!(
            error.to_string().contains("duplicate entry"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn load_bundle_rejects_backslash_traversal() -> TestResult {
        let bytes = build_zip(&[
            (BUNDLE_MANIFEST_NAME, MANIFEST),
            ("..\\..\\escape.txt", "malicious"),
        ])?;
        let error = load_bundle(&bytes)
            .err()
            .ok_or("expected a backslash-path rejection")?;
        assert!(
            error.to_string().contains("backslash"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn load_bundle_enforces_actual_decompressed_size_not_metadata() -> TestResult {
        // A highly compressible entry stays tiny on disk but decompresses past
        // the per-file limit; the limit must be enforced against actual bytes.
        let oversized = "a".repeat(9 * 1024 * 1024);
        let bytes = build_zip(&[
            (BUNDLE_MANIFEST_NAME, MANIFEST),
            ("skills/my-skill/big.txt", oversized.as_str()),
        ])?;
        assert!(
            bytes.len() < 1024 * 1024,
            "expected the compressed bundle to stay small, got {} bytes",
            bytes.len()
        );
        let error = load_bundle(&bytes)
            .err()
            .ok_or("expected a decompressed-size rejection")?;
        assert!(
            error.to_string().contains("per-file limit"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn load_bundle_merges_multiple_yaml_documents() -> TestResult {
        const AGENT: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Agent
metadata:
  name: helper
spec:
  name: Helper
  harness: acp
  systemPrompt: be helpful
";
        let bytes = build_zip(&[
            ("skill.yaml", MANIFEST),
            ("config/agent.yml", AGENT),
            ("skills/my-skill/SKILL.md", "# hello"),
        ])?;
        let document = load_bundle(&bytes)?;
        assert_eq!(document.spec.skills.len(), 1, "expected the skill document");
        assert_eq!(document.spec.agents.len(), 1, "expected the agent document");
        assert_eq!(document.spec.agents[0].id, "helper");
        Ok(())
    }

    #[test]
    fn load_bundle_resolves_source_relative_to_declaring_yaml() -> TestResult {
        // A manifest under config/ points at a sibling-relative skill directory;
        // the resolved directory's files (including a skill-owned *.yaml) become
        // skill content, while the manifest itself never does.
        const RELATIVE_MANIFEST: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: ../skills/my-skill
";
        let bytes = build_zip(&[
            ("config/skill.yaml", RELATIVE_MANIFEST),
            ("skills/my-skill/SKILL.md", "# hello"),
            ("skills/my-skill/scripts/run.sh", "echo hi"),
        ])?;
        let document = load_bundle(&bytes)?;
        let skill = document
            .spec
            .skills
            .first()
            .ok_or("expected one skill in the bundle document")?;
        assert_eq!(skill.id, "my-skill");
        assert_eq!(
            skill.files.get("SKILL.md").map(String::as_str),
            Some("# hello")
        );
        assert_eq!(
            skill.files.get("scripts/run.sh").map(String::as_str),
            Some("echo hi")
        );
        Ok(())
    }

    #[test]
    fn load_bundle_treats_skill_owned_yaml_as_content_not_manifest() -> TestResult {
        // A YAML file that lives inside a skill source directory must be skill
        // content, never parsed as a configuration manifest.
        const ROOT_MANIFEST: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: skills/my-skill
";
        let bytes = build_zip(&[
            ("agentspace-config.yaml", ROOT_MANIFEST),
            ("skills/my-skill/SKILL.md", "# hello"),
            ("skills/my-skill/config.yaml", "arbitrary: not-a-manifest"),
        ])?;
        let document = load_bundle(&bytes)?;
        let skill = document
            .spec
            .skills
            .first()
            .ok_or("expected one skill in the bundle document")?;
        assert_eq!(
            skill.files.get("config.yaml").map(String::as_str),
            Some("arbitrary: not-a-manifest"),
            "skill-owned yaml must be materialized as content"
        );
        Ok(())
    }

    #[test]
    fn load_bundle_supports_direct_file_source_path() -> TestResult {
        // spec.source.path may point directly at a single file; the skill then
        // has one file keyed by that file's basename.
        const DIRECT_FILE_MANIFEST: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: skills/my-skill/SKILL.md
";
        let bytes = build_zip(&[
            (BUNDLE_MANIFEST_NAME, DIRECT_FILE_MANIFEST),
            ("skills/my-skill/SKILL.md", "# only file"),
        ])?;
        let document = load_bundle(&bytes)?;
        let skill = document
            .spec
            .skills
            .first()
            .ok_or("expected one skill in the bundle document")?;
        assert_eq!(skill.files.len(), 1, "expected a single-file skill");
        assert_eq!(
            skill.files.get("SKILL.md").map(String::as_str),
            Some("# only file")
        );
        Ok(())
    }

    #[test]
    fn load_bundle_excludes_config_nested_skill_yaml_from_manifests() -> TestResult {
        // A skill source directory nested under config/ may contain its own
        // *.yaml files; those must be skill content, never config manifests,
        // even though they live under the config/ manifest prefix.
        const NESTED_MANIFEST: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: skill
";
        let bytes = build_zip(&[
            ("config/manifest.yaml", NESTED_MANIFEST),
            ("config/skill/SKILL.md", "# hello"),
            ("config/skill/rules.yaml", "arbitrary: skill-content"),
        ])?;
        let document = load_bundle(&bytes)?;
        assert_eq!(document.spec.skills.len(), 1, "expected exactly one skill");
        let skill = &document.spec.skills[0];
        assert_eq!(
            skill.files.get("rules.yaml").map(String::as_str),
            Some("arbitrary: skill-content"),
            "config/-nested skill yaml must be treated as skill content"
        );
        Ok(())
    }

    #[test]
    fn load_bundle_rejects_source_escaping_declaring_yaml() -> TestResult {
        const ESCAPING_MANIFEST: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: ../../secrets
";
        let bytes = build_zip(&[("config/skill.yaml", ESCAPING_MANIFEST)])?;
        let error = load_bundle(&bytes)
            .err()
            .ok_or("expected a bundle-root escape rejection")?;
        assert!(
            error.to_string().contains("escapes the bundle root"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn load_bundle_rejects_missing_source_files() -> TestResult {
        let bytes = build_zip(&[(BUNDLE_MANIFEST_NAME, MANIFEST)])?;
        assert!(load_bundle(&bytes).is_err());
        Ok(())
    }

    #[test]
    fn load_bundle_rejects_path_traversal() -> TestResult {
        let bytes = build_zip(&[
            (BUNDLE_MANIFEST_NAME, MANIFEST),
            ("../escape.txt", "malicious"),
        ])?;
        let error = load_bundle(&bytes)
            .err()
            .ok_or("expected a path-traversal rejection")?;
        assert!(
            error.to_string().contains("escapes the archive root"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn load_bundle_rejects_absolute_source_path() -> TestResult {
        const ABSOLUTE_MANIFEST: &str = r"apiVersion: agentspace.dev/v1alpha1
kind: Skill
metadata:
  name: my-skill
spec:
  source:
    path: /etc/passwd
";
        let bytes = build_zip(&[(BUNDLE_MANIFEST_NAME, ABSOLUTE_MANIFEST)])?;
        assert!(load_bundle(&bytes).is_err());
        Ok(())
    }
}
