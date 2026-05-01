# ruff: noqa: S603,S607
from __future__ import annotations

import os
import shutil
import subprocess
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import cast

from git_agent.patch_parser import EMPTY_TREE_SHA, NULL_SHA


class GitCommandError(RuntimeError):
    def __init__(self, args: Sequence[str], stderr: str) -> None:
        self.args_list = tuple(args)
        self.stderr = stderr
        msg = f"git command failed: {' '.join(args)}: {stderr}"
        super().__init__(msg)


class PatchApplyError(RuntimeError):
    pass


class RefUpdateError(RuntimeError):
    pass


@dataclass(frozen=True, kw_only=True)
class GitResult:
    returncode: int
    stdout: str
    stderr: str


@dataclass(frozen=True, kw_only=True)
class RefInfo:
    ref: str
    sha: str


@dataclass(frozen=True, kw_only=True)
class GitHttpResponse:
    status_code: int
    headers: dict[str, str]
    body: bytes


@dataclass(frozen=True)
class PreparedPatch:
    request_id: str
    scratch_dir: Path
    worktree: Path
    review_worktree: Path | None
    index_path: Path
    base_sha: str | None
    repo_path: Path

    def cleanup(self) -> None:
        if self.review_worktree is not None:
            _remove_registered_worktree(self.repo_path, self.review_worktree)
        if self.scratch_dir.exists():
            shutil.rmtree(self.scratch_dir)


@dataclass(frozen=True, kw_only=True)
class ValidationResult:
    ok: bool
    stdout: str
    stderr: str
    returncode: int


class GitBackend:
    def __init__(self, repo_path: Path, scratch_path: Path) -> None:
        self.repo_path = repo_path
        self.scratch_path = scratch_path

    @property
    def repo_name(self) -> str:
        return self.repo_path.name

    def initialize(self) -> None:
        self.repo_path.parent.mkdir(parents=True, exist_ok=True)
        self.scratch_path.mkdir(parents=True, exist_ok=True)
        if not self.repo_path.exists():
            _run_git(["init", "--bare", "--initial-branch=main", str(self.repo_path)])
        elif not (self.repo_path / "HEAD").exists():
            if any(self.repo_path.iterdir()):
                msg = (
                    "repo path exists but is not a bare git repository: "
                    f"{self.repo_path}"
                )
                raise RuntimeError(msg)
            _run_git(["init", "--bare", "--initial-branch=main", str(self.repo_path)])
        _run_git(
            ["--git-dir", str(self.repo_path), "config", "http.receivepack", "false"],
        )

    def status(self) -> dict[str, object]:
        head_ref = _run_git(
            ["--git-dir", str(self.repo_path), "symbolic-ref", "-q", "HEAD"],
            check=False,
        ).stdout.strip()
        refs = [ref.__dict__ for ref in self.list_refs()]
        return {
            "repo_path": str(self.repo_path),
            "repo_name": self.repo_name,
            "head_ref": head_ref or None,
            "empty": not refs,
            "refs": refs,
        }

    def list_refs(self) -> list[RefInfo]:
        result = _run_git(
            [
                "--git-dir",
                str(self.repo_path),
                "for-each-ref",
                "--format=%(refname)%09%(objectname)",
            ],
        )
        refs: list[RefInfo] = []
        for line in result.stdout.splitlines():
            ref, sha = line.split("\t", maxsplit=1)
            refs.append(RefInfo(ref=ref, sha=sha))
        return refs

    def get_ref(self, ref: str) -> str | None:
        result = _run_git(
            [
                "--git-dir",
                str(self.repo_path),
                "rev-parse",
                "--verify",
                f"{ref}^{{commit}}",
            ],
            check=False,
        )
        if result.returncode != 0:
            return None
        return result.stdout.strip()

    def commit_exists(self, sha: str) -> bool:
        result = _run_git(
            ["--git-dir", str(self.repo_path), "cat-file", "-e", f"{sha}^{{commit}}"],
            check=False,
        )
        return result.returncode == 0

    def prepare_patch(
        self,
        *,
        request_id: str,
        base_sha: str | None,
        raw_patch: str,
        create_review_worktree: bool = False,
    ) -> PreparedPatch:
        scratch_dir = self.scratch_path / request_id
        if scratch_dir.exists():
            shutil.rmtree(scratch_dir)
        worktree = scratch_dir / "worktree"
        worktree.mkdir(parents=True)
        index_path = scratch_dir / "index"
        env = self._patch_env(worktree=worktree, index_path=index_path)

        if base_sha is None:
            _run_git(["read-tree", "--empty"], env=env)
        else:
            read_result = _run_git(
                ["read-tree", "--reset", base_sha],
                env=env,
                check=False,
            )
            if read_result.returncode != 0:
                _cleanup_path(scratch_dir)
                raise PatchApplyError(
                    read_result.stderr.strip() or read_result.stdout.strip(),
                )

        apply_result = _run_git(
            ["apply", "--cached", "--binary", "-"],
            env=env,
            input_text=raw_patch,
            check=False,
        )
        if apply_result.returncode != 0:
            _cleanup_path(scratch_dir)
            raise PatchApplyError(
                apply_result.stderr.strip() or apply_result.stdout.strip(),
            )

        diff_result = _run_git(
            ["diff", "--cached", "--quiet", "--exit-code"],
            env=env,
            check=False,
        )
        if diff_result.returncode == 0:
            _cleanup_path(scratch_dir)
            msg = "patch applies but produces no changes"
            raise PatchApplyError(msg)
        if diff_result.returncode != 1:
            _cleanup_path(scratch_dir)
            raise PatchApplyError(
                diff_result.stderr.strip() or diff_result.stdout.strip(),
            )

        checkout_result = _run_git(
            ["checkout-index", "-a", "-f"],
            env=env,
            cwd=worktree,
            check=False,
        )
        if checkout_result.returncode != 0:
            _cleanup_path(scratch_dir)
            raise PatchApplyError(
                checkout_result.stderr.strip() or checkout_result.stdout.strip(),
            )

        review_worktree: Path | None = None
        if create_review_worktree:
            try:
                review_worktree = self._prepare_review_worktree(
                    scratch_dir=scratch_dir,
                    base_sha=base_sha,
                    raw_patch=raw_patch,
                )
            except PatchApplyError:
                _cleanup_path(scratch_dir)
                raise

        return PreparedPatch(
            request_id=request_id,
            scratch_dir=scratch_dir,
            worktree=worktree,
            review_worktree=review_worktree,
            index_path=index_path,
            base_sha=base_sha,
            repo_path=self.repo_path,
        )

    def _prepare_review_worktree(
        self,
        *,
        scratch_dir: Path,
        base_sha: str | None,
        raw_patch: str,
    ) -> Path:
        review_worktree = scratch_dir / "review-worktree"
        review_base_sha = base_sha or self._create_empty_review_base_commit()
        add_result = _run_git(
            [
                "--git-dir",
                str(self.repo_path),
                "worktree",
                "add",
                "--detach",
                str(review_worktree),
                review_base_sha,
            ],
            check=False,
        )
        if add_result.returncode != 0:
            raise PatchApplyError(
                add_result.stderr.strip() or add_result.stdout.strip(),
            )

        apply_result = _run_git(
            ["-C", str(review_worktree), "apply", "--index", "--binary", "-"],
            input_text=raw_patch,
            check=False,
        )
        if apply_result.returncode != 0:
            _remove_registered_worktree(self.repo_path, review_worktree)
            raise PatchApplyError(
                apply_result.stderr.strip() or apply_result.stdout.strip(),
            )

        diff_result = _run_git(
            ["-C", str(review_worktree), "diff", "--quiet", "--exit-code", "HEAD"],
            check=False,
        )
        if diff_result.returncode == 0:
            _remove_registered_worktree(self.repo_path, review_worktree)
            msg = "patch applies but produces no review worktree changes"
            raise PatchApplyError(msg)
        if diff_result.returncode != 1:
            _remove_registered_worktree(self.repo_path, review_worktree)
            raise PatchApplyError(
                diff_result.stderr.strip() or diff_result.stdout.strip(),
            )
        return review_worktree

    def _create_empty_review_base_commit(self) -> str:
        env = _git_identity_env()
        return _run_git(
            [
                "--git-dir",
                str(self.repo_path),
                "commit-tree",
                EMPTY_TREE_SHA,
                "-m",
                "GitAgent empty review base",
            ],
            env=env,
        ).stdout.strip()

    def commit_prepared_patch(
        self,
        *,
        prepared: PreparedPatch,
        target_ref: str,
        expected_old: str | None,
        message: str,
        author: object | None,
    ) -> str:
        env = self._patch_env(
            worktree=prepared.worktree,
            index_path=prepared.index_path,
            author=author,
        )
        tree_sha = _run_git(["write-tree"], env=env).stdout.strip()
        commit_args = ["commit-tree", tree_sha]
        if prepared.base_sha is not None:
            commit_args.extend(["-p", prepared.base_sha])
        commit_args.extend(["-m", message])
        commit_sha = _run_git(commit_args, env=env).stdout.strip()
        old_value = expected_old or NULL_SHA
        result = _run_git(
            [
                "--git-dir",
                str(self.repo_path),
                "update-ref",
                "-m",
                f"PatchRequest {prepared.request_id}",
                target_ref,
                commit_sha,
                old_value,
            ],
            check=False,
        )
        if result.returncode != 0:
            raise RefUpdateError(result.stderr.strip() or result.stdout.strip())
        return commit_sha

    def run_validation(
        self,
        *,
        prepared: PreparedPatch,
        command: Sequence[str],
        timeout_seconds: float,
    ) -> ValidationResult:
        completed = subprocess.run(
            list(command),
            cwd=prepared.worktree,
            text=True,
            capture_output=True,
            timeout=timeout_seconds,
            check=False,
        )
        return ValidationResult(
            ok=completed.returncode == 0,
            stdout=completed.stdout,
            stderr=completed.stderr,
            returncode=completed.returncode,
        )

    def run_http_backend(
        self,
        *,
        path_info: str,
        query_string: str,
        method: str,
        body: bytes,
        content_type: str | None,
    ) -> GitHttpResponse:
        _validate_http_path(path_info, self.repo_name)
        if _is_receive_pack(path_info, query_string):
            return GitHttpResponse(
                status_code=403,
                headers={"content-type": "text/plain; charset=utf-8"},
                body=b"receive-pack is disabled; submit patches to /PatchRequest\n",
            )
        if not _is_upload_pack(path_info, query_string):
            return GitHttpResponse(
                status_code=403,
                headers={"content-type": "text/plain; charset=utf-8"},
                body=b"only git-upload-pack is enabled\n",
            )

        env = os.environ.copy()
        env.update(
            {
                "GIT_PROJECT_ROOT": str(self.repo_path.parent),
                "GIT_HTTP_EXPORT_ALL": "1",
                "PATH_INFO": path_info,
                "QUERY_STRING": query_string,
                "REQUEST_METHOD": method,
                "CONTENT_LENGTH": str(len(body)),
            },
        )
        if content_type is not None:
            env["CONTENT_TYPE"] = content_type
        completed = subprocess.run(
            ["git", "http-backend"],
            input=body,
            capture_output=True,
            env=env,
            check=False,
        )
        if completed.returncode != 0:
            return GitHttpResponse(
                status_code=500,
                headers={"content-type": "text/plain; charset=utf-8"},
                body=completed.stderr,
            )
        return _parse_cgi_response(completed.stdout)

    def _patch_env(
        self,
        *,
        worktree: Path,
        index_path: Path,
        author: object | None = None,
    ) -> dict[str, str]:
        env = os.environ.copy()
        env.update(
            {
                "GIT_DIR": str(self.repo_path),
                "GIT_WORK_TREE": str(worktree),
                "GIT_INDEX_FILE": str(index_path),
                "GIT_AUTHOR_NAME": "GitAgent",
                "GIT_AUTHOR_EMAIL": "gitagent@example.invalid",
                "GIT_COMMITTER_NAME": "GitAgent",
                "GIT_COMMITTER_EMAIL": "gitagent@example.invalid",
            },
        )
        author_env = _author_env(author)
        env.update(author_env)
        return env


def _run_git(
    args: Sequence[str],
    *,
    env: Mapping[str, str] | None = None,
    cwd: Path | None = None,
    input_text: str | None = None,
    check: bool = True,
) -> GitResult:
    completed = subprocess.run(
        ["git", *args],
        input=input_text,
        text=True,
        capture_output=True,
        cwd=cwd,
        env=dict(env) if env is not None else None,
        check=False,
    )
    result = GitResult(
        returncode=completed.returncode,
        stdout=completed.stdout,
        stderr=completed.stderr,
    )
    if check and completed.returncode != 0:
        raise GitCommandError(["git", *args], completed.stderr.strip())
    return result


def _author_env(author: object | None) -> dict[str, str]:
    if not isinstance(author, Mapping):
        if isinstance(author, str) and author.strip():
            return {"GIT_AUTHOR_NAME": author.strip()}
        return {}
    mapping = cast("Mapping[str, object]", author)
    result: dict[str, str] = {}
    name = mapping.get("name")
    email = mapping.get("email")
    if isinstance(name, str) and name.strip():
        result["GIT_AUTHOR_NAME"] = name.strip()
    if isinstance(email, str) and email.strip():
        result["GIT_AUTHOR_EMAIL"] = email.strip()
    return result


def _git_identity_env() -> dict[str, str]:
    env = os.environ.copy()
    env.update(
        {
            "GIT_AUTHOR_NAME": "GitAgent",
            "GIT_AUTHOR_EMAIL": "gitagent@example.invalid",
            "GIT_COMMITTER_NAME": "GitAgent",
            "GIT_COMMITTER_EMAIL": "gitagent@example.invalid",
        },
    )
    return env


def _remove_registered_worktree(repo_path: Path, worktree: Path) -> None:
    if not worktree.exists():
        return
    _run_git(
        [
            "--git-dir",
            str(repo_path),
            "worktree",
            "remove",
            "--force",
            str(worktree),
        ],
        check=False,
    )


def _cleanup_path(path: Path) -> None:
    if path.exists():
        shutil.rmtree(path)


def _validate_http_path(path_info: str, repo_name: str) -> None:
    path = path_info.removeprefix("/")
    pure = PurePosixPath(path)
    if pure.is_absolute() or ".." in pure.parts or not pure.parts:
        msg = "invalid git HTTP path"
        raise ValueError(msg)
    if pure.parts[0] != repo_name:
        msg = "unknown git repository"
        raise FileNotFoundError(msg)


def _is_receive_pack(path_info: str, query_string: str) -> bool:
    return (
        path_info.endswith("/git-receive-pack")
        or "service=git-receive-pack" in query_string
    )


def _is_upload_pack(path_info: str, query_string: str) -> bool:
    return (
        path_info.endswith("/git-upload-pack")
        or "service=git-upload-pack" in query_string
    )


def _parse_cgi_response(raw: bytes) -> GitHttpResponse:
    if b"\r\n\r\n" in raw:
        raw_headers, body = raw.split(b"\r\n\r\n", maxsplit=1)
    elif b"\n\n" in raw:
        raw_headers, body = raw.split(b"\n\n", maxsplit=1)
    else:
        return GitHttpResponse(status_code=200, headers={}, body=raw)

    status_code = 200
    headers: dict[str, str] = {}
    for line in raw_headers.decode("latin-1").splitlines():
        if not line:
            continue
        name, _, value = line.partition(":")
        if not value:
            continue
        if name.lower() == "status":
            status_code = int(value.strip().split(" ", maxsplit=1)[0])
        else:
            headers[name.lower()] = value.strip()
    return GitHttpResponse(status_code=status_code, headers=headers, body=body)
