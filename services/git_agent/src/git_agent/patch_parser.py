from __future__ import annotations

import re
import shlex
from dataclasses import dataclass, field
from pathlib import PurePosixPath
from typing import Literal

CommentSide = Literal["left", "right", "binary", "general"]

_HUNK_RE = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
_WIP_REF_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._/-]*$")
_DEV_NULL_PATH = "/dev/null"
NULL_SHA = "0" * 40
EMPTY_TREE_SHA = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"


class PatchValidationError(ValueError):
    pass


@dataclass(frozen=True, order=True)
class ChangedLine:
    path: str
    side: Literal["left", "right"]
    line: int

    def to_json(self) -> dict[str, object]:
        return {"path": self.path, "side": self.side, "line": self.line}


def _new_str_set() -> set[str]:
    return set()


def _new_changed_line_set() -> set[ChangedLine]:
    return set()


@dataclass
class PatchAnalysis:
    paths: set[str] = field(default_factory=_new_str_set)
    binary_paths: set[str] = field(default_factory=_new_str_set)
    changed_lines: set[ChangedLine] = field(default_factory=_new_changed_line_set)

    @property
    def has_binary(self) -> bool:
        return bool(self.binary_paths)

    def line_indexes_json(self) -> list[dict[str, object]]:
        return [line.to_json() for line in sorted(self.changed_lines)]


def normalize_target_ref(raw_ref: str) -> str:
    ref = raw_ref.strip()
    if ref == "main":
        return "refs/heads/main"
    if ref == "refs/heads/main":
        return ref
    if ref.startswith("wip/"):
        ref = f"refs/heads/{ref}"
    if ref.startswith("refs/heads/wip/"):
        name = ref.removeprefix("refs/heads/wip/")
        if not _safe_wip_ref_name(name):
            msg = "wip ref names may contain only safe git path characters"
            raise PatchValidationError(msg)
        return ref
    msg = (
        "target_ref must be main, refs/heads/main, wip/<name>, or refs/heads/wip/<name>"
    )
    raise PatchValidationError(msg)


def is_protected_ref(ref: str) -> bool:
    return ref == "refs/heads/main"


def validate_sha(value: str) -> str:
    sha = value.strip().lower()
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        msg = "base_sha must be a full 40-character hexadecimal commit id"
        raise PatchValidationError(msg)
    return sha


def is_null_sha(value: str | None) -> bool:
    return value is None or value == NULL_SHA


def is_empty_base_sha(value: str | None) -> bool:
    return value is None or value in {NULL_SHA, EMPTY_TREE_SHA}


def analyze_patch(raw_patch: str) -> PatchAnalysis:  # noqa: C901, PLR0912, PLR0915
    analysis = PatchAnalysis()
    current_old: str | None = None
    current_new: str | None = None
    old_line = 0
    new_line = 0
    in_hunk = False

    for line in raw_patch.splitlines():
        if line.startswith("diff --git "):
            current_old, current_new = _parse_diff_git_paths(line)
            for path in (current_old, current_new):
                if path is not None:
                    analysis.paths.add(path)
            in_hunk = False
            continue

        if line.startswith("--- "):
            current_old = _parse_marker_path(line[4:])
            if current_old is not None:
                analysis.paths.add(current_old)
            continue

        if line.startswith("+++ "):
            current_new = _parse_marker_path(line[4:])
            if current_new is not None:
                analysis.paths.add(current_new)
            continue

        if line.startswith("Binary files ") or line == "GIT binary patch":
            binary_path = current_new or current_old
            if binary_path is not None:
                analysis.binary_paths.add(binary_path)
                analysis.paths.add(binary_path)
            in_hunk = False
            continue

        match = _HUNK_RE.match(line)
        if match:
            if current_old is None and current_new is None:
                msg = "hunk is missing file path headers"
                raise PatchValidationError(msg)
            old_line = int(match.group(1))
            new_line = int(match.group(3))
            in_hunk = True
            continue

        if not in_hunk:
            continue

        if line.startswith("\\"):
            continue
        if line.startswith("+"):
            if current_new is None:
                msg = "added line is missing a new file path"
                raise PatchValidationError(msg)
            analysis.changed_lines.add(ChangedLine(current_new, "right", new_line))
            new_line += 1
            continue
        if line.startswith("-"):
            if current_old is None:
                msg = "deleted line is missing an old file path"
                raise PatchValidationError(msg)
            analysis.changed_lines.add(ChangedLine(current_old, "left", old_line))
            old_line += 1
            continue
        if line.startswith(" "):
            old_line += 1
            new_line += 1
            continue

        in_hunk = False

    if not analysis.paths:
        msg = "patch must contain at least one file diff"
        raise PatchValidationError(msg)
    return analysis


def validate_patch_paths(analysis: PatchAnalysis) -> None:
    for path in analysis.paths:
        _validate_safe_path(path)


def normalize_patch_path(path: str) -> str:
    normalized = _strip_diff_prefix(path.strip())
    return _validate_safe_path(normalized)


def _safe_wip_ref_name(name: str) -> bool:
    if not name or name.endswith(("/", ".lock")):
        return False
    if ".." in name or "//" in name or "\\" in name:
        return False
    if any(ord(char) < 32 or char in " ~^:?*[" for char in name):
        return False
    return bool(_WIP_REF_RE.fullmatch(name))


def _parse_diff_git_paths(line: str) -> tuple[str | None, str | None]:
    try:
        parts = shlex.split(line)
    except ValueError as exc:
        msg = "invalid diff --git path quoting"
        raise PatchValidationError(msg) from exc
    if len(parts) < 4:
        msg = "diff --git line must include old and new paths"
        raise PatchValidationError(msg)
    return _parse_path_token(parts[2]), _parse_path_token(parts[3])


def _parse_marker_path(raw: str) -> str | None:
    marker_path = raw.split("\t", maxsplit=1)[0].strip()
    return _parse_path_token(marker_path)


def _parse_path_token(path_token: str) -> str | None:
    if path_token == _DEV_NULL_PATH:
        return None
    return normalize_patch_path(path_token)


def _strip_diff_prefix(path: str) -> str:
    if path.startswith(("a/", "b/")):
        return path[2:]
    return path


def _validate_safe_path(path: str) -> str:
    if not path or path == ".":
        msg = "patch paths must not be empty"
        raise PatchValidationError(msg)
    if "\x00" in path or "\\" in path:
        msg = f"unsafe patch path: {path}"
        raise PatchValidationError(msg)
    if path.startswith("/") or "//" in path:
        msg = f"unsafe patch path: {path}"
        raise PatchValidationError(msg)
    pure = PurePosixPath(path)
    if pure.is_absolute() or ".." in pure.parts or ".git" in pure.parts:
        msg = f"unsafe patch path: {path}"
        raise PatchValidationError(msg)
    return pure.as_posix()
