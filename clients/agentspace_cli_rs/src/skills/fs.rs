use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use super::error::SkillsError;

pub fn collect_skill_directory(
    skill_dir: &Path,
) -> Result<(String, BTreeMap<String, String>), SkillsError> {
    if !skill_dir.is_dir() {
        return Err(SkillsError::InvalidSkillDirectory {
            path: skill_dir.to_owned(),
        });
    }
    let skill_id = skill_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SkillsError::NonUtf8Path {
            path: skill_dir.to_owned(),
        })?
        .to_owned();
    if !valid_skill_id(&skill_id) {
        return Err(SkillsError::InvalidSkillId { skill_id });
    }

    let mut files = BTreeMap::new();
    collect(skill_dir, skill_dir, &mut files)?;
    let manifest = skill_dir.join("SKILL.md");
    if !files.contains_key("SKILL.md") {
        return Err(SkillsError::MissingManifest { path: manifest });
    }
    Ok((skill_id, files))
}

fn collect(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), SkillsError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| SkillsError::Io {
            path: directory.to_owned(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SkillsError::Io {
            path: directory.to_owned(),
            source,
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| SkillsError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(SkillsError::Symlink { path });
        }
        if metadata.is_dir() {
            collect(root, &path, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(SkillsError::UnsupportedFile { path });
        }
        let relative = relative_path(root, &path)?;
        let bytes = fs::read(&path).map_err(|source| SkillsError::Io {
            path: path.clone(),
            source,
        })?;
        let content = String::from_utf8(bytes)
            .map_err(|_| SkillsError::NonUtf8Content { path: path.clone() })?;
        files.insert(relative, content);
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> Result<String, SkillsError> {
    path.strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .map(|relative| relative.replace(std::path::MAIN_SEPARATOR, "/"))
        .ok_or_else(|| SkillsError::NonUtf8Path {
            path: PathBuf::from(path),
        })
}

fn valid_skill_id(skill_id: &str) -> bool {
    !skill_id.is_empty()
        && skill_id.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn recursively_collects_utf8_files() {
        let root = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let skill = root.path().join("weather-report");
        fs::create_dir_all(skill.join("scripts"))
            .unwrap_or_else(|error| panic!("create directories: {error}"));
        fs::write(skill.join("SKILL.md"), "# Weather\n")
            .unwrap_or_else(|error| panic!("write manifest: {error}"));
        fs::write(skill.join("scripts/forecast.sh"), "echo sunny\n")
            .unwrap_or_else(|error| panic!("write script: {error}"));

        let (skill_id, files) = collect_skill_directory(&skill)
            .unwrap_or_else(|error| panic!("collect skill: {error}"));

        assert_eq!(skill_id, "weather-report");
        assert_eq!(
            files,
            BTreeMap::from([
                ("SKILL.md".to_owned(), "# Weather\n".to_owned()),
                ("scripts/forecast.sh".to_owned(), "echo sunny\n".to_owned()),
            ])
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let skill = root.path().join("linked-skill");
        fs::create_dir(&skill).unwrap_or_else(|error| panic!("create skill: {error}"));
        fs::write(skill.join("SKILL.md"), "# Linked\n")
            .unwrap_or_else(|error| panic!("write manifest: {error}"));
        symlink(skill.join("SKILL.md"), skill.join("linked.md"))
            .unwrap_or_else(|error| panic!("create symlink: {error}"));

        assert!(matches!(
            collect_skill_directory(&skill),
            Err(SkillsError::Symlink { .. })
        ));
    }

    #[test]
    fn requires_manifest_and_valid_directory_name() {
        let root = tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let missing = root.path().join("missing-manifest");
        fs::create_dir(&missing).unwrap_or_else(|error| panic!("create skill: {error}"));
        assert!(matches!(
            collect_skill_directory(&missing),
            Err(SkillsError::MissingManifest { .. })
        ));

        let invalid = root.path().join("Invalid_Name");
        fs::create_dir(&invalid).unwrap_or_else(|error| panic!("create skill: {error}"));
        assert!(matches!(
            collect_skill_directory(&invalid),
            Err(SkillsError::InvalidSkillId { .. })
        ));
    }
}
