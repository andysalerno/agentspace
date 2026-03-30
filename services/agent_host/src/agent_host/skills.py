"""Filesystem-backed skills CRUD.

Skills are stored on a volume mounted into the agent-host container.
Each skill is a directory containing files (primarily markdown).
The volume path is configured via AGENT_HOST_SKILLS_DIR (default: /skills).

Builtin skills can be loaded from a second directory (e.g. a bind-mounted
repo folder at /builtin-skills). They are copied into the main skills
directory at startup and marked read-only via the API.
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


class BuiltinSkillReadOnlyError(ValueError):
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

    def __init__(
        self,
        skills_dir: str | None = None,
        builtin_skills_dir: str | None = None,
    ) -> None:
        self._skills_dir = Path(
            skills_dir or os.environ.get("AGENT_HOST_SKILLS_DIR", "/skills"),
        )
        self._builtin_skills_dir = Path(
            builtin_skills_dir
            or os.environ.get("AGENT_HOST_BUILTIN_SKILLS_DIR", "/builtin-skills"),
        )
        self._builtin_ids: set[str] = set()

    def sync_builtin_skills(self) -> None:
        """Copy skills from the builtin directory into the main skills dir.

        Each subdirectory of the builtin dir whose name matches the skill ID
        pattern is copied (overwritten) into the main skills directory. The
        set of builtin skill IDs is recorded so the API can mark them
        read-only.
        """
        if not self._builtin_skills_dir.is_dir():
            logger.info(
                "builtin skills dir %s not found, skipping sync",
                self._builtin_skills_dir,
            )
            return

        self._ensure_base_dir()
        synced: list[str] = []

        for entry in sorted(self._builtin_skills_dir.iterdir()):
            if not entry.is_dir():
                continue
            if not SKILL_ID_PATTERN.fullmatch(entry.name):
                logger.warning(
                    "skipping builtin skill with invalid id: %s",
                    entry.name,
                )
                continue
            dest = self._skills_dir / entry.name
            if dest.exists():
                shutil.rmtree(dest)
            shutil.copytree(entry, dest)
            self._builtin_ids.add(entry.name)
            synced.append(entry.name)

        logger.info("synced %d builtin skill(s): %s", len(synced), synced)

    def is_builtin(self, skill_id: str) -> bool:
        return skill_id in self._builtin_ids

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
        if self.is_builtin(skill_id):
            raise SkillAlreadyExistsError(skill_id)
        self._ensure_base_dir()
        skill_dir = self._skill_path(skill_id)
        if skill_dir.exists():
            raise SkillAlreadyExistsError(skill_id)
        skill_dir.mkdir(parents=True)
        self._write_skill_files(skill_dir, files)
        logger.info("created skill %s with %d files", skill_id, len(files))
        return {
            "skill_id": skill_id,
            "files": self._read_skill_files(skill_dir),
            "source": "user",
        }

    def get_skill(self, skill_id: str) -> SkillDict:
        _validate_skill_id(skill_id)
        skill_dir = self._skill_path(skill_id)
        if not skill_dir.is_dir():
            raise SkillNotFoundError(skill_id)
        return {
            "skill_id": skill_id,
            "files": self._read_skill_files(skill_dir),
            "source": "builtin" if self.is_builtin(skill_id) else "user",
        }

    def list_skills(self) -> list[SkillDict]:
        self._ensure_base_dir()
        return [
            {
                "skill_id": entry.name,
                "source": "builtin" if self.is_builtin(entry.name) else "user",
            }
            for entry in sorted(self._skills_dir.iterdir())
            if entry.is_dir() and SKILL_ID_PATTERN.fullmatch(entry.name)
        ]

    def update_skill(self, skill_id: str, files: dict[str, str]) -> SkillDict:
        _validate_skill_id(skill_id)
        if self.is_builtin(skill_id):
            msg = f"builtin skill '{skill_id}' is read-only"
            raise BuiltinSkillReadOnlyError(msg)
        skill_dir = self._skill_path(skill_id)
        if not skill_dir.is_dir():
            raise SkillNotFoundError(skill_id)
        # Remove existing files and replace
        shutil.rmtree(skill_dir)
        skill_dir.mkdir(parents=True)
        self._write_skill_files(skill_dir, files)
        logger.info("updated skill %s with %d files", skill_id, len(files))
        return {
            "skill_id": skill_id,
            "files": self._read_skill_files(skill_dir),
            "source": "user",
        }

    def delete_skill(self, skill_id: str) -> None:
        _validate_skill_id(skill_id)
        if self.is_builtin(skill_id):
            msg = f"builtin skill '{skill_id}' is read-only"
            raise BuiltinSkillReadOnlyError(msg)
        skill_dir = self._skill_path(skill_id)
        if not skill_dir.is_dir():
            raise SkillNotFoundError(skill_id)
        shutil.rmtree(skill_dir)
        logger.info("deleted skill %s", skill_id)
