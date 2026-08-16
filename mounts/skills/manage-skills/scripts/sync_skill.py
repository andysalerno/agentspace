# ruff: noqa: EM101, EM102, INP001, TRY003

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Protocol

SKILL_ID = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")


class RequestJson(Protocol):
    def __call__(
        self,
        method: str,
        url: str,
        payload: dict[str, Any] | None = None,
    ) -> tuple[int, dict[str, Any]]: ...


def collect_files(skill_dir: Path) -> dict[str, str]:
    if not skill_dir.is_dir():
        raise ValueError(f"skill directory does not exist: {skill_dir}")

    files: dict[str, str] = {}
    for path in sorted(skill_dir.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"skill files cannot be symlinks: {path}")
        if not path.is_file():
            continue
        relative = path.relative_to(skill_dir).as_posix()
        try:
            files[relative] = path.read_text(encoding="utf-8")
        except UnicodeDecodeError as error:
            raise ValueError(f"skill files must be UTF-8 text: {path}") from error

    if "SKILL.md" not in files:
        raise ValueError(f"{skill_dir} must contain SKILL.md")
    return files


def request_json(
    method: str,
    url: str,
    payload: dict[str, Any] | None = None,
) -> tuple[int, dict[str, Any]]:
    data = None if payload is None else json.dumps(payload).encode()
    headers = {} if data is None else {"Content-Type": "application/json"}
    request = urllib.request.Request(  # noqa: S310
        url,
        data=data,
        headers=headers,
        method=method,
    )
    try:
        with urllib.request.urlopen(request) as response:  # noqa: S310
            body = response.read()
            return response.status, json.loads(body) if body else {}
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")
        if error.code == 404:
            return error.code, {}
        message = body or error.reason
        raise RuntimeError(
            f"{method} {url} failed ({error.code}): {message}",
        ) from error


def sync_skill(
    skill_dir: Path,
    api_url: str,
    agent_id: str | None,
    request: RequestJson = request_json,
) -> dict[str, Any]:
    skill_id = skill_dir.name
    if SKILL_ID.fullmatch(skill_id) is None:
        raise ValueError(
            "skill directory name must contain lowercase letters, numbers, "
            "and single hyphens only",
        )

    files = collect_files(skill_dir)
    skill_url = f"{api_url.rstrip('/')}/{skill_id}"
    status, existing = request("GET", skill_url)
    if status == 404:
        payload: dict[str, Any] = {"skill_id": skill_id, "files": files}
        if agent_id:
            payload["creator_agent_id"] = agent_id
        _, result = request("POST", api_url.rstrip("/"), payload)
        return result

    if existing.get("source") != "user":
        raise ValueError(f"refusing to update non-user skill {skill_id!r}")
    _, result = request("PUT", skill_url, {"files": files})
    return result


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Create or update an AgentSpace skill from a local directory.",
    )
    parser.add_argument("skill_dir", type=Path)
    args = parser.parse_args()

    api_url = os.environ.get("AGENTSPACE_SKILLS_API")
    if not api_url:
        parser.error("AGENTSPACE_SKILLS_API is not set")

    try:
        result = sync_skill(
            args.skill_dir.resolve(),
            api_url,
            os.environ.get("AGENTSPACE_AGENT_ID"),
        )
    except (OSError, RuntimeError, ValueError) as error:
        sys.stderr.write(f"error: {error}\n")
        return 1

    sys.stdout.write(f"{json.dumps(result, indent=2, sort_keys=True)}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
