from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, cast

ReviewMode = Literal["client", "auto_accept", "auto_reject", "invalid"]


@dataclass(frozen=True, kw_only=True)
class Settings:
    repo_path: Path = Path("/data/git-agent/repo.git")
    db_path: Path = Path("/data/git-agent/git_agent.sqlite3")
    scratch_path: Path = Path("/data/git-agent/worktrees")
    data_path: Path = Path("/data")
    review_workspace_mount_path: Path = Path("/workspace/git-agent")
    review_mode: ReviewMode = "auto_reject"
    review_agent_id: str | None = None
    client_service_url: str = "http://client-service:8002"
    validation_command: tuple[str, ...] | None = None
    validation_timeout_seconds: float = 300.0

    @classmethod
    def from_env(cls) -> Settings:
        repo_path = Path(os.environ.get("GITAGENT_REPO_PATH", cls.repo_path.as_posix()))
        db_path = Path(os.environ.get("GITAGENT_DB_PATH", cls.db_path.as_posix()))
        scratch_path = Path(
            os.environ.get("GITAGENT_SCRATCH_PATH", cls.scratch_path.as_posix()),
        )
        data_path = Path(os.environ.get("GITAGENT_DATA_PATH", cls.data_path.as_posix()))
        review_workspace_mount_path = Path(
            os.environ.get(
                "GITAGENT_REVIEW_WORKSPACE_MOUNT_PATH",
                cls.review_workspace_mount_path.as_posix(),
            ),
        )
        review_agent_id = os.environ.get("GITAGENT_REVIEW_AGENT_ID") or None
        raw_mode = os.environ.get("GITAGENT_REVIEW_MODE")
        review_mode: ReviewMode
        if raw_mode is None:
            review_mode = "client" if review_agent_id else "auto_reject"
        elif raw_mode in {"client", "auto_accept", "auto_reject", "invalid"}:
            review_mode = cast("ReviewMode", raw_mode)
        else:
            msg = (
                "GITAGENT_REVIEW_MODE must be client, auto_accept, auto_reject, "
                "or invalid"
            )
            raise ValueError(msg)

        validation_command = _validation_command_from_env()
        timeout = float(os.environ.get("GITAGENT_VALIDATION_TIMEOUT_SECONDS", "300"))
        return cls(
            repo_path=repo_path,
            db_path=db_path,
            scratch_path=scratch_path,
            data_path=data_path,
            review_workspace_mount_path=review_workspace_mount_path,
            review_mode=review_mode,
            review_agent_id=review_agent_id,
            client_service_url=os.environ.get(
                "GITAGENT_CLIENT_SERVICE_URL",
                cls.client_service_url,
            ),
            validation_command=validation_command,
            validation_timeout_seconds=timeout,
        )


def _validation_command_from_env() -> tuple[str, ...] | None:
    raw = os.environ.get("GITAGENT_VALIDATION_COMMAND_JSON")
    if raw:
        parsed = json.loads(raw)
        if not isinstance(parsed, list) or not parsed:
            msg = "GITAGENT_VALIDATION_COMMAND_JSON must be a non-empty JSON array"
            raise ValueError(msg)
        items = cast("list[object]", parsed)
        command: list[str] = []
        for item in items:
            if not isinstance(item, str) or not item:
                msg = "validation command entries must be non-empty strings"
                raise ValueError(msg)
            command.append(item)
        return tuple(command)

    if os.environ.get("GITAGENT_VALIDATE") == "1":
        return ("just", "validate")
    return None
