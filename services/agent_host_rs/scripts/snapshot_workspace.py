# ruff: noqa: INP001

from __future__ import annotations

import json
import os
import pathlib
import shutil

source = pathlib.Path(
    os.environ.get("AGENTSPACE_WORKSPACE_SOURCE", "/workspace-src"),
)
dest = pathlib.Path(
    os.environ.get("AGENTSPACE_WORKSPACE_DEST", "/workspace-dest"),
)
profile_marker = b"<!-- agentspace-owned-profile:"
skills_staging = pathlib.PurePosixPath("/mnt/all-skills")


def validated_relative_path(raw: str) -> pathlib.PurePosixPath:
    path = pathlib.PurePosixPath(raw)
    if (
        not raw
        or "\\" in raw
        or path.is_absolute()
        or any(not part or part in {".", ".."} for part in raw.split("/"))
    ):
        msg = f"invalid workspace exclusion path: {raw!r}"
        raise ValueError(msg)
    return path


exclude = {
    validated_relative_path(item)
    for item in json.loads(
        os.environ.get("AGENTSPACE_WORKSPACE_EXCLUDE_PATHS_JSON", "[]"),
    )
}


def remove_existing(path: pathlib.Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)


def reject_symlink_ancestors(
    base: pathlib.Path,
    relative: pathlib.PurePosixPath,
) -> None:
    current = base
    for part in relative.parts[:-1]:
        current /= part
        if current.is_symlink():
            msg = f"cannot safely exclude a path through symlink ancestor: {relative}"
            raise ValueError(msg)


def is_managed_artifact_path(relative: pathlib.PurePosixPath) -> bool:
    parts = relative.parts
    return len(parts) == 3 and (
        parts[:2] == (".github", "skills")
        or (
            parts[:2] == (".github", "agents")
            and parts[2].startswith("agentspace-")
            and parts[2].endswith(".agent.md")
        )
    )


def is_owned_artifact(
    path: pathlib.Path,
    relative: pathlib.PurePosixPath,
) -> bool:
    parts = relative.parts
    if parts[:2] == (".github", "skills"):
        if not path.is_symlink():
            return False
        target = pathlib.PurePosixPath(path.readlink())
        return target.is_absolute() and (
            target == skills_staging or skills_staging in target.parents
        )
    if parts[:2] == (".github", "agents") and not path.is_symlink() and path.is_file():
        try:
            return profile_marker in path.read_bytes()
        except OSError:
            return False
    return False


def should_exclude(
    path: pathlib.Path,
    relative: pathlib.PurePosixPath,
) -> bool:
    if is_managed_artifact_path(relative):
        return is_owned_artifact(path, relative)
    return relative in exclude


def remove_owned_artifacts(root: pathlib.Path) -> None:
    if (root / ".github").is_symlink():
        return
    for relative_dir in (
        pathlib.PurePosixPath(".github/agents"),
        pathlib.PurePosixPath(".github/skills"),
    ):
        directory = root.joinpath(*relative_dir.parts)
        if not directory.is_dir() or directory.is_symlink():
            continue
        for entry in directory.iterdir():
            relative = relative_dir / entry.name
            if is_managed_artifact_path(relative) and is_owned_artifact(
                entry,
                relative,
            ):
                remove_existing(entry)


def copy_entry(
    entry: pathlib.Path,
    target: pathlib.Path,
    relative: pathlib.PurePosixPath,
) -> None:
    if should_exclude(entry, relative):
        remove_existing(target)
        return
    if entry.is_symlink():
        remove_existing(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.symlink_to(entry.readlink())
    elif entry.is_dir():
        if target.is_symlink() or (target.exists() and not target.is_dir()):
            remove_existing(target)
        target.mkdir(parents=True, exist_ok=True)
        for child in entry.iterdir():
            copy_entry(child, target / child.name, relative / child.name)
    else:
        remove_existing(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(entry, target)


dest.mkdir(parents=True, exist_ok=True)
remove_owned_artifacts(dest)
for excluded_path in exclude:
    reject_symlink_ancestors(source, excluded_path)
    reject_symlink_ancestors(dest, excluded_path)
    source_candidate = source.joinpath(*excluded_path.parts)
    target_candidate = dest.joinpath(*excluded_path.parts)
    candidate = (
        source_candidate
        if source_candidate.exists() or source_candidate.is_symlink()
        else target_candidate
    )
    if should_exclude(candidate, excluded_path):
        remove_existing(target_candidate)

for source_entry in source.iterdir():
    copy_entry(
        source_entry,
        dest / source_entry.name,
        pathlib.PurePosixPath(source_entry.name),
    )
