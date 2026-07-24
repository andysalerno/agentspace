//! Skill-content validation shared by the config validator and interactive
//! skill CRUD.
//!
//! The rules mirror the authoritative checks in `agent_host` so a skill that
//! validates here is one that `agent_host` will also accept: every skill must
//! contain a `SKILL.md`, file paths must be safe relative paths, and an
//! optional `agentspace.json` must parse against schema version 1 with
//! normalized, non-reserved volume mount paths. Mount paths must also be unique
//! across every skill in the document so two enabled skills cannot collide.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

/// The required entry-point document for every skill.
pub const SKILL_MARKDOWN_FILE: &str = "SKILL.md";
/// The optional metadata document describing skill resources.
pub const SKILL_METADATA_FILE: &str = "agentspace.json";

/// Absolute paths that skill volume mounts may never overlap.
const RESERVED_MOUNT_PATHS: &[&str] = &[
    "/workspace",
    "/root/.copilot",
    "/mnt/all-skills",
    "/skills",
    "/root/.config/opencode/skills",
];

/// A single validation failure discovered in a skill's content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillContentIssue {
    pub code: &'static str,
    pub message: String,
    /// Field path relative to the skill (for example `files/SKILL.md`).
    pub field: Option<String>,
}

impl SkillContentIssue {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            field: None,
        }
    }

    fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillMetadata {
    schema_version: u32,
    #[serde(default)]
    resources: SkillResources,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillResources {
    #[serde(default)]
    volumes: Vec<SkillVolumeDeclaration>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillVolumeDeclaration {
    id: String,
    scope: SkillVolumeScope,
    mount_path: String,
    #[serde(default)]
    #[allow(dead_code)]
    advertise: bool,
    mode: SkillVolumeMode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SkillVolumeScope {
    Installation,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SkillVolumeMode {
    Ro,
    Rw,
}

/// Validate that a relative file path is safe: non-empty, not absolute, free of
/// `..` traversal, and naming an actual file component.
#[must_use]
pub fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    if std::path::Path::new(path).is_absolute() {
        return false;
    }
    // Windows drive-absolute form such as `C:foo`.
    if let Some((prefix, _)) = path.split_once(':')
        && prefix.len() == 1
        && prefix.chars().all(|c| c.is_ascii_alphabetic())
    {
        return false;
    }
    let mut has_file_component = false;
    for component in path.split('/') {
        match component {
            ".." => return false,
            "" | "." => {}
            _ => has_file_component = true,
        }
    }
    has_file_component
}

/// Validate a normalized absolute volume mount path that must not overlap a
/// reserved kernel path.
#[must_use]
fn is_valid_mount_path(mount_path: &str) -> bool {
    let normalized = mount_path.starts_with('/')
        && mount_path != "/"
        && !mount_path.ends_with('/')
        && mount_path
            .split('/')
            .skip(1)
            .all(|component| !component.is_empty() && component != "." && component != "..");
    if !normalized {
        return false;
    }
    !RESERVED_MOUNT_PATHS
        .iter()
        .any(|reserved| mount_path == *reserved || mount_path.starts_with(&format!("{reserved}/")))
}

/// Validate a single skill's content, appending any issues discovered.
///
/// Volume mount paths declared by this skill are inserted into `mount_paths`
/// (mapping mount path to declaring skill id) so callers can detect collisions
/// across every skill in the document.
pub fn validate_skill_content(
    issues: &mut Vec<SkillContentIssue>,
    skill_id: &str,
    files: &BTreeMap<String, String>,
    mount_paths: &mut BTreeMap<String, String>,
) {
    if !files.contains_key(SKILL_MARKDOWN_FILE) {
        issues.push(
            SkillContentIssue::new(
                "missing_skill_markdown",
                format!("skill {skill_id:?} must contain a {SKILL_MARKDOWN_FILE} document"),
            )
            .with_field(format!("skills/{skill_id}/files/{SKILL_MARKDOWN_FILE}")),
        );
    }

    for path in files.keys() {
        if !is_safe_relative_path(path) {
            issues.push(
                SkillContentIssue::new(
                    "invalid_skill_file_path",
                    format!("skill {skill_id:?} file path {path:?} is not a safe relative path"),
                )
                .with_field(format!("skills/{skill_id}/files")),
            );
        }
    }

    let Some(metadata_content) = files.get(SKILL_METADATA_FILE) else {
        return;
    };
    let metadata = match serde_json::from_str::<SkillMetadata>(metadata_content) {
        Ok(metadata) => metadata,
        Err(error) => {
            issues.push(
                SkillContentIssue::new(
                    "invalid_skill_metadata",
                    format!("skill {skill_id:?} {SKILL_METADATA_FILE} is invalid: {error}"),
                )
                .with_field(format!("skills/{skill_id}/files/{SKILL_METADATA_FILE}")),
            );
            return;
        }
    };
    if metadata.schema_version != 1 {
        issues.push(
            SkillContentIssue::new(
                "invalid_skill_metadata",
                format!(
                    "skill {skill_id:?} {SKILL_METADATA_FILE} has unsupported schemaVersion {}; \
                     expected 1",
                    metadata.schema_version
                ),
            )
            .with_field(format!("skills/{skill_id}/files/{SKILL_METADATA_FILE}")),
        );
        return;
    }

    let mut resource_ids = BTreeSet::new();
    for volume in &metadata.resources.volumes {
        let SkillVolumeScope::Installation = volume.scope;
        let (SkillVolumeMode::Ro | SkillVolumeMode::Rw) = volume.mode;
        if !resource_ids.insert(volume.id.clone()) {
            issues.push(
                SkillContentIssue::new(
                    "invalid_skill_metadata",
                    format!(
                        "skill {skill_id:?} declares duplicate volume resource id {:?}",
                        volume.id
                    ),
                )
                .with_field(format!("skills/{skill_id}/files/{SKILL_METADATA_FILE}")),
            );
        }
        if !is_valid_mount_path(&volume.mount_path) {
            issues.push(
                SkillContentIssue::new(
                    "invalid_skill_mount_path",
                    format!(
                        "skill {skill_id:?} volume mount path {:?} must be a normalized absolute \
                         path that does not overlap a reserved kernel path",
                        volume.mount_path
                    ),
                )
                .with_field(format!("skills/{skill_id}/files/{SKILL_METADATA_FILE}")),
            );
            continue;
        }
        if let Some(previous) = mount_paths.insert(volume.mount_path.clone(), skill_id.to_owned()) {
            issues.push(
                SkillContentIssue::new(
                    "skill_mount_path_collision",
                    format!(
                        "skill {skill_id:?} volume mount path {:?} collides with skill {previous:?}",
                        volume.mount_path
                    ),
                )
                .with_field(format!("skills/{skill_id}/files/{SKILL_METADATA_FILE}")),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, content)| ((*name).to_owned(), (*content).to_owned()))
            .collect()
    }

    #[test]
    fn accepts_a_minimal_skill() {
        let mut issues = Vec::new();
        let mut mounts = BTreeMap::new();
        validate_skill_content(
            &mut issues,
            "my-skill",
            &files(&[("SKILL.md", "# hi")]),
            &mut mounts,
        );
        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
    }

    #[test]
    fn requires_skill_markdown() {
        let mut issues = Vec::new();
        let mut mounts = BTreeMap::new();
        validate_skill_content(
            &mut issues,
            "my-skill",
            &files(&[("notes.txt", "hi")]),
            &mut mounts,
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "missing_skill_markdown")
        );
    }

    #[test]
    fn rejects_traversal_paths() {
        assert!(!is_safe_relative_path("../escape"));
        assert!(!is_safe_relative_path("/abs"));
        assert!(!is_safe_relative_path("a\\b"));
        assert!(is_safe_relative_path("tools/run.sh"));
        assert!(is_safe_relative_path("./docs/intro.md"));
    }

    #[test]
    fn rejects_reserved_mount_paths() {
        let metadata = r#"{"schema_version":1,"resources":{"volumes":[
            {"id":"cache","scope":"installation","mount_path":"/skills/x","mode":"rw"}]}}"#;
        let mut issues = Vec::new();
        let mut mounts = BTreeMap::new();
        validate_skill_content(
            &mut issues,
            "my-skill",
            &files(&[("SKILL.md", "# hi"), (SKILL_METADATA_FILE, metadata)]),
            &mut mounts,
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "invalid_skill_mount_path")
        );
    }

    #[test]
    fn detects_cross_skill_mount_collision() {
        let metadata = r#"{"schema_version":1,"resources":{"volumes":[
            {"id":"cache","scope":"installation","mount_path":"/data/shared","mode":"rw"}]}}"#;
        let mut issues = Vec::new();
        let mut mounts = BTreeMap::new();
        validate_skill_content(
            &mut issues,
            "skill-a",
            &files(&[("SKILL.md", "# a"), (SKILL_METADATA_FILE, metadata)]),
            &mut mounts,
        );
        validate_skill_content(
            &mut issues,
            "skill-b",
            &files(&[("SKILL.md", "# b"), (SKILL_METADATA_FILE, metadata)]),
            &mut mounts,
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "skill_mount_path_collision")
        );
    }

    #[test]
    fn rejects_bad_metadata_schema() {
        let metadata = r#"{"schema_version":2}"#;
        let mut issues = Vec::new();
        let mut mounts = BTreeMap::new();
        validate_skill_content(
            &mut issues,
            "my-skill",
            &files(&[("SKILL.md", "# hi"), (SKILL_METADATA_FILE, metadata)]),
            &mut mounts,
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "invalid_skill_metadata")
        );
    }
}
