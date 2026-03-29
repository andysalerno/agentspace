"""Filesystem-backed skills CRUD.

Skills are stored on a volume mounted into the agent-host container.
Each skill is a directory containing files (primarily markdown).
The volume path is configured via AGENT_HOST_SKILLS_DIR (default: /skills).
"""

from __future__ import annotations

import logging
import os
import re
import shutil
from pathlib import Path, PurePosixPath
from typing import Any

logger = logging.getLogger(__name__)

SKILL_ID_PATTERN = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")

type SkillDict = dict[str, Any]


class SkillNotFoundError(KeyError):
    pass


class SkillAlreadyExistsError(ValueError):
    pass


class InvalidSkillIdError(ValueError):
    pass


class InvalidSkillFilePathError(ValueError):
    pass


def _validate_skill_id(skill_id: str) -> None:
    if not SKILL_ID_PATTERN.fullmatch(skill_id):
        msg = (
            "skill_id must use lowercase alphanumeric"
            " characters and single hyphens only"
        )
        raise InvalidSkillIdError(msg)


def _validate_file_path(relative_path: str) -> None:
    """Reject paths that could escape the skill directory."""
    pure = PurePosixPath(relative_path)
    if pure.is_absolute():
        msg = f"file path must be relative: {relative_path}"
        raise InvalidSkillFilePathError(msg)
    if ".." in pure.parts:
        msg = f"file path must not contain '..': {relative_path}"
        raise InvalidSkillFilePathError(msg)
    if not relative_path or relative_path.startswith("/"):
        msg = f"invalid file path: {relative_path}"
        raise InvalidSkillFilePathError(msg)


class SkillsService:
    """Filesystem-backed skill management."""

    def __init__(self, skills_dir: str | None = None) -> None:
        self._skills_dir = Path(
            skills_dir or os.environ.get("AGENT_HOST_SKILLS_DIR", "/skills"),
        )

    def _ensure_base_dir(self) -> None:
        self._skills_dir.mkdir(parents=True, exist_ok=True)

    def _skill_path(self, skill_id: str) -> Path:
        return self._skills_dir / skill_id

    def _read_skill_files(self, skill_dir: Path) -> dict[str, str]:
        files: dict[str, str] = {}
        for file_path in sorted(skill_dir.rglob("*")):
            if file_path.is_file():
                relative = file_path.relative_to(skill_dir).as_posix()
                files[relative] = file_path.read_text(encoding="utf-8")
        return files

    def _write_skill_files(self, skill_dir: Path, files: dict[str, str]) -> None:
        for relative_path, content in files.items():
            _validate_file_path(relative_path)
            file_path = skill_dir / relative_path
            file_path.parent.mkdir(parents=True, exist_ok=True)
            file_path.write_text(content, encoding="utf-8")

    def create_skill(self, skill_id: str, files: dict[str, str]) -> SkillDict:
        _validate_skill_id(skill_id)
        self._ensure_base_dir()
        skill_dir = self._skill_path(skill_id)
        if skill_dir.exists():
            raise SkillAlreadyExistsError(skill_id)
        skill_dir.mkdir(parents=True)
        self._write_skill_files(skill_dir, files)
        logger.info("created skill %s with %d files", skill_id, len(files))
        return {"skill_id": skill_id, "files": self._read_skill_files(skill_dir)}

    def get_skill(self, skill_id: str) -> SkillDict:
        _validate_skill_id(skill_id)
        skill_dir = self._skill_path(skill_id)
        if not skill_dir.is_dir():
            raise SkillNotFoundError(skill_id)
        return {
            "skill_id": skill_id,
            "files": self._read_skill_files(skill_dir),
        }

    def list_skills(self) -> list[SkillDict]:
        self._ensure_base_dir()
        return [
            {"skill_id": entry.name}
            for entry in sorted(self._skills_dir.iterdir())
            if entry.is_dir() and SKILL_ID_PATTERN.fullmatch(entry.name)
        ]

    def update_skill(self, skill_id: str, files: dict[str, str]) -> SkillDict:
        _validate_skill_id(skill_id)
        skill_dir = self._skill_path(skill_id)
        if not skill_dir.is_dir():
            raise SkillNotFoundError(skill_id)
        # Remove existing files and replace
        shutil.rmtree(skill_dir)
        skill_dir.mkdir(parents=True)
        self._write_skill_files(skill_dir, files)
        logger.info("updated skill %s with %d files", skill_id, len(files))
        return {"skill_id": skill_id, "files": self._read_skill_files(skill_dir)}

    def delete_skill(self, skill_id: str) -> None:
        _validate_skill_id(skill_id)
        skill_dir = self._skill_path(skill_id)
        if not skill_dir.is_dir():
            raise SkillNotFoundError(skill_id)
        shutil.rmtree(skill_dir)
        logger.info("deleted skill %s", skill_id)
