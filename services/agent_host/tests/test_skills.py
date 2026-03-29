from __future__ import annotations

import pytest
from agent_host.skills import (
    InvalidSkillFilePathError,
    InvalidSkillIdError,
    SkillAlreadyExistsError,
    SkillNotFoundError,
    SkillsService,
)


@pytest.fixture
def skills_service(tmp_path: object) -> SkillsService:
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
