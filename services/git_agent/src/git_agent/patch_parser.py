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


def _canonical_head_ref(ref: str) -> str:
    """Return the canonical full ref for a branch reference.

    A bare branch name (``wip/x``) is expanded to ``refs/heads/wip/x``. A ref
    that already carries a ``refs/`` prefix (``refs/heads/wip/x``) is returned
    unchanged so the ``refs/heads/`` prefix is never doubled up.
    """
    if ref.startswith("refs/"):
        return ref
    return f"refs/heads/{ref}"


def _branch_relative(prefix: str) -> str:
    """Normalize an allowed prefix to its branch-relative form.

    Both ``wip/`` and ``refs/heads/wip/`` normalize to ``wip/`` so a policy may
    be authored with or without the ``refs/heads/`` prefix without changing
    matching behavior.
    """
    return prefix.removeprefix("refs/heads/")


def normalize_target_ref(
    raw_ref: str,
    *,
    default_branch: str = "main",
    allowed_prefixes: tuple[str, ...] = ("wip/",),
    allowed_refs: tuple[str, ...] = (),
) -> str:
    """Normalize and authorize a target ref against the configured policy.

    The default branch is always permitted (protected). Exact ``allowed_refs``
    are matched against the canonical full ref, and ``allowed_prefixes`` match
    the branch-relative portion. Inputs may be bare (``wip/x``) or full
    (``refs/heads/wip/x``); the canonical full ref is returned in every case and
    the ``refs/heads/`` prefix is never doubled. An empty prefix and ref policy
    permits only the default branch (it is never implicitly permissive).
    """
    ref = raw_ref.strip()
    if not ref:
        msg = "target_ref must not be empty"
        raise PatchValidationError(msg)

    protected = _canonical_head_ref(default_branch)
    full = _canonical_head_ref(ref)
    if full == protected:
        return protected

    # Exact allowed refs are honored as-is (matched on the canonical full ref).
    for allowed in allowed_refs:
        if full == _canonical_head_ref(allowed.strip()):
            return full

    # Prefix matches operate on the branch-relative name so a heads-only policy
    # is enforced and the remainder (user-controlled) is validated.
    if full.startswith("refs/heads/"):
        name = full.removeprefix("refs/heads/")
        for prefix in allowed_prefixes:
            relative = _branch_relative(prefix)
            if not relative or not name.startswith(relative):
                continue
            remainder = name.removeprefix(relative)
            if not _safe_wip_ref_name(remainder):
                msg = "branch names may contain only safe git path characters"
                raise PatchValidationError(msg)
            return full

    allowed_desc: list[str] = [default_branch, protected]
    allowed_desc.extend(sorted(allowed_refs))
    allowed_desc.extend(
        f"{_branch_relative(prefix)}<name>" for prefix in allowed_prefixes
    )
    msg = f"target_ref must be one of: {', '.join(allowed_desc)}"
    raise PatchValidationError(msg)


def is_protected_ref(ref: str, *, default_branch: str = "main") -> bool:
    return _canonical_head_ref(ref) == _canonical_head_ref(default_branch)


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
