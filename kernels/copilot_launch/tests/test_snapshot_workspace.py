from __future__ import annotations

import json
import runpy
from pathlib import Path

import pytest

SNAPSHOT_SCRIPT = (
    Path(__file__).parents[3] / "services/agent_host_rs/scripts/snapshot_workspace.py"
)


def _run_snapshot(
    monkeypatch: pytest.MonkeyPatch,
    source: Path,
    dest: Path,
    exclude_paths: list[str],
) -> None:
    monkeypatch.setenv("AGENTSPACE_WORKSPACE_SOURCE", str(source))
    monkeypatch.setenv("AGENTSPACE_WORKSPACE_DEST", str(dest))
    monkeypatch.setenv(
        "AGENTSPACE_WORKSPACE_EXCLUDE_PATHS_JSON",
        json.dumps(exclude_paths),
    )
    runpy.run_path(str(SNAPSHOT_SCRIPT), run_name="__main__")


def test_snapshot_excludes_owned_nested_artifacts_only(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    dest = tmp_path / "dest"
    profile_dir = source / ".github/agents"
    skills_dir = source / ".github/skills"
    profile_dir.mkdir(parents=True)
    skills_dir.mkdir(parents=True)
    owned_profile = profile_dir / "agentspace-session.agent.md"
    owned_profile.write_text(
        "---\ndescription: owned\n---\n"
        "<!-- agentspace-owned-profile:agentspace-session -->\n",
        encoding="utf-8",
    )
    user_profile = profile_dir / "reviewer.agent.md"
    user_profile.write_text("user profile", encoding="utf-8")
    (source / ".github/settings.yml").write_text("user settings", encoding="utf-8")
    (skills_dir / "alpha").symlink_to("/mnt/all-skills/alpha")
    user_skill = skills_dir / "beta"
    user_skill.mkdir()
    (user_skill / "SKILL.md").write_text("user skill", encoding="utf-8")

    _run_snapshot(
        monkeypatch,
        source,
        dest,
        [
            ".github/skills/beta",
        ],
    )

    assert not (dest / ".github/agents/agentspace-session.agent.md").exists()
    assert (dest / ".github/agents/reviewer.agent.md").read_text() == "user profile"
    assert (dest / ".github/settings.yml").read_text() == "user settings"
    assert not (dest / ".github/skills/alpha").exists()
    assert (dest / ".github/skills/beta/SKILL.md").read_text() == "user skill"


def test_snapshot_excludes_top_level_paths(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    dest = tmp_path / "dest"
    (source / "mounted-workspace").mkdir(parents=True)
    (source / "mounted-workspace/file.txt").write_text("mounted", encoding="utf-8")
    (source / "keep.txt").write_text("keep", encoding="utf-8")

    _run_snapshot(monkeypatch, source, dest, ["mounted-workspace"])

    assert not (dest / "mounted-workspace").exists()
    assert (dest / "keep.txt").read_text() == "keep"


def test_snapshot_removes_stale_owned_artifact_from_destination(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    dest = tmp_path / "dest"
    source.mkdir()
    stale_profile = dest / ".github/agents/agentspace-session.agent.md"
    stale_profile.parent.mkdir(parents=True)
    stale_profile.write_text(
        "<!-- agentspace-owned-profile:agentspace-session -->\n",
        encoding="utf-8",
    )

    _run_snapshot(
        monkeypatch,
        source,
        dest,
        [".github/agents/agentspace-session.agent.md"],
    )

    assert not stale_profile.exists()


def test_snapshot_rejects_nested_exclusion_through_symlink(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    source = tmp_path / "source"
    dest = tmp_path / "dest"
    external = tmp_path / "external"
    source.mkdir()
    external.mkdir()
    (source / ".github").symlink_to(external)

    with pytest.raises(ValueError, match="symlink ancestor"):
        _run_snapshot(
            monkeypatch,
            source,
            dest,
            [".github/skills/alpha"],
        )
