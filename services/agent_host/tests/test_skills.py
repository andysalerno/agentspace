from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from agent_host.skills import (
    BuiltinSkillReadOnlyError,
    InvalidSkillFilePathError,
    InvalidSkillIdError,
    SkillAlreadyExistsError,
    SkillNotFoundError,
    SkillsService,
)

if TYPE_CHECKING:
    from pathlib import Path


@pytest.fixture
def skills_service(tmp_path: Path) -> SkillsService:
    return SkillsService(skills_dir=str(tmp_path))


def test_create_and_get_skill(skills_service: SkillsService) -> None:
    files = {"SKILL.md": "# My Skill\nDoes things.", "extra.md": "Extra info."}
    created = skills_service.create_skill("my-skill", files)

    assert created["skill_id"] == "my-skill"
    assert created["files"]["SKILL.md"] == "# My Skill\nDoes things."
    assert created["files"]["extra.md"] == "Extra info."

    fetched = skills_service.get_skill("my-skill")
    assert fetched == created


def test_list_skills(skills_service: SkillsService) -> None:
    skills_service.create_skill("alpha-skill", {"SKILL.md": "# Alpha"})
    skills_service.create_skill("beta-skill", {"SKILL.md": "# Beta"})

    result = skills_service.list_skills()

    assert [s["skill_id"] for s in result] == ["alpha-skill", "beta-skill"]


def test_update_skill(skills_service: SkillsService) -> None:
    skills_service.create_skill("my-skill", {"SKILL.md": "# V1"})

    updated = skills_service.update_skill(
        "my-skill",
        {"SKILL.md": "# V2", "new-file.md": "New content."},
    )

    assert updated["files"]["SKILL.md"] == "# V2"
    assert updated["files"]["new-file.md"] == "New content."
    assert len(updated["files"]) == 2


def test_delete_skill(skills_service: SkillsService) -> None:
    skills_service.create_skill("my-skill", {"SKILL.md": "# Doomed"})
    skills_service.delete_skill("my-skill")

    with pytest.raises(SkillNotFoundError):
        skills_service.get_skill("my-skill")

    assert skills_service.list_skills() == []


def test_create_duplicate_raises(skills_service: SkillsService) -> None:
    skills_service.create_skill("my-skill", {"SKILL.md": "# First"})

    with pytest.raises(SkillAlreadyExistsError):
        skills_service.create_skill("my-skill", {"SKILL.md": "# Second"})


def test_get_missing_raises(skills_service: SkillsService) -> None:
    with pytest.raises(SkillNotFoundError):
        skills_service.get_skill("nonexistent")


def test_update_missing_raises(skills_service: SkillsService) -> None:
    with pytest.raises(SkillNotFoundError):
        skills_service.update_skill("nonexistent", {"SKILL.md": "# Nope"})


def test_delete_missing_raises(skills_service: SkillsService) -> None:
    with pytest.raises(SkillNotFoundError):
        skills_service.delete_skill("nonexistent")


def test_invalid_skill_id_raises(skills_service: SkillsService) -> None:
    with pytest.raises(InvalidSkillIdError):
        skills_service.create_skill("Bad Skill", {"SKILL.md": "# Bad"})

    with pytest.raises(InvalidSkillIdError):
        skills_service.create_skill("../escape", {"SKILL.md": "# Bad"})


def test_invalid_file_path_raises(skills_service: SkillsService) -> None:
    with pytest.raises(InvalidSkillFilePathError):
        skills_service.create_skill("my-skill", {"../escape.md": "# Bad"})

    with pytest.raises(InvalidSkillFilePathError):
        skills_service.create_skill("ok-skill", {"/absolute.md": "# Bad"})


def test_nested_files(skills_service: SkillsService) -> None:
    files = {
        "SKILL.md": "# Nested Skill",
        "tools/helper.py": "print('hello')",
    }
    created = skills_service.create_skill("nested-skill", files)

    assert created["files"]["tools/helper.py"] == "print('hello')"


# --- Builtin skills ---


@pytest.fixture
def builtin_service(tmp_path: Path) -> SkillsService:
    skills_dir = tmp_path / "skills"
    builtin_dir = tmp_path / "builtin"
    builtin_dir.mkdir()

    # Create two builtin skills on disk
    ws = builtin_dir / "websearch"
    ws.mkdir()
    (ws / "SKILL.md").write_text("# Websearch\nSearches the web.")
    (ws / "search.sh").write_text("#!/bin/sh\ncurl $1")

    news = builtin_dir / "news"
    news.mkdir()
    (news / "SKILL.md").write_text("# News\nFetches news.")

    svc = SkillsService(skills_dir=str(skills_dir), builtin_skills_dir=str(builtin_dir))
    svc.sync_builtin_skills()
    return svc


def test_sync_copies_builtin_skills(builtin_service: SkillsService) -> None:
    listed = builtin_service.list_skills()
    ids = [s["skill_id"] for s in listed]
    assert "websearch" in ids
    assert "news" in ids


def test_builtin_skills_have_source_builtin(builtin_service: SkillsService) -> None:
    listed = builtin_service.list_skills()
    for skill in listed:
        assert skill["source"] == "builtin"

    detail = builtin_service.get_skill("websearch")
    assert detail["source"] == "builtin"
    assert "SKILL.md" in detail["files"]
    assert "search.sh" in detail["files"]


def test_builtin_update_rejected(builtin_service: SkillsService) -> None:
    with pytest.raises(BuiltinSkillReadOnlyError):
        builtin_service.update_skill("websearch", {"SKILL.md": "# Hacked"})


def test_builtin_delete_rejected(builtin_service: SkillsService) -> None:
    with pytest.raises(BuiltinSkillReadOnlyError):
        builtin_service.delete_skill("websearch")


def test_builtin_create_duplicate_rejected(builtin_service: SkillsService) -> None:
    with pytest.raises(SkillAlreadyExistsError):
        builtin_service.create_skill("websearch", {"SKILL.md": "# Dup"})


def test_user_skills_alongside_builtins(builtin_service: SkillsService) -> None:
    builtin_service.create_skill("my-custom", {"SKILL.md": "# Custom"})

    listed = builtin_service.list_skills()
    sources = {s["skill_id"]: s["source"] for s in listed}
    assert sources["websearch"] == "builtin"
    assert sources["news"] == "builtin"
    assert sources["my-custom"] == "user"


def test_user_skill_has_source_user(skills_service: SkillsService) -> None:
    created = skills_service.create_skill("my-skill", {"SKILL.md": "# Mine"})
    assert created["source"] == "user"

    detail = skills_service.get_skill("my-skill")
    assert detail["source"] == "user"


def test_sync_overwrites_existing_skill(tmp_path: Path) -> None:
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    builtin_dir = tmp_path / "builtin"
    builtin_dir.mkdir()

    # Pre-existing skill with old content
    old = skills_dir / "websearch"
    old.mkdir()
    (old / "SKILL.md").write_text("# Old version")

    # Builtin with new content
    ws = builtin_dir / "websearch"
    ws.mkdir()
    (ws / "SKILL.md").write_text("# New version")

    svc = SkillsService(skills_dir=str(skills_dir), builtin_skills_dir=str(builtin_dir))
    svc.sync_builtin_skills()

    detail = svc.get_skill("websearch")
    assert detail["files"]["SKILL.md"] == "# New version"
    assert detail["source"] == "builtin"


def test_sync_skips_invalid_dir_names(tmp_path: Path) -> None:
    skills_dir = tmp_path / "skills"
    builtin_dir = tmp_path / "builtin"
    builtin_dir.mkdir()

    bad = builtin_dir / "Bad Name"
    bad.mkdir()
    (bad / "SKILL.md").write_text("# Bad")

    svc = SkillsService(skills_dir=str(skills_dir), builtin_skills_dir=str(builtin_dir))
    svc.sync_builtin_skills()

    assert svc.list_skills() == []


def test_sync_with_missing_builtin_dir(tmp_path: Path) -> None:
    skills_dir = tmp_path / "skills"
    svc = SkillsService(
        skills_dir=str(skills_dir),
        builtin_skills_dir=str(tmp_path / "nonexistent"),
    )
    svc.sync_builtin_skills()  # should not raise
    assert svc.list_skills() == []
