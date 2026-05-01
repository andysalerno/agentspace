from __future__ import annotations

import os
import subprocess
from pathlib import Path


def test_helper_clone_initializes_empty_checkout_for_first_run(tmp_path: Path) -> None:
    repo_root = Path(__file__).resolve().parents[3]
    helper = repo_root / "mounts/skills/gitagent-helper/gitagent-helper.sh"
    target = tmp_path / "repo"
    remote = tmp_path / "missing.git"
    env = os.environ.copy()
    env.update(
        {
            "GITAGENT_REMOTE_URL": remote.as_posix(),
            "GITAGENT_DEFAULT_BRANCH": "main",
        },
    )

    completed = subprocess.run(  # noqa: S603
        ["/bin/bash", str(helper), "clone", str(target)],
        cwd=tmp_path,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )

    assert completed.returncode == 0, completed.stderr
    assert "initialized an empty local checkout" in completed.stderr
    assert _run(["git", "-C", str(target), "remote", "get-url", "origin"]) == (
        remote.as_posix()
    )
    assert _run(["git", "-C", str(target), "branch", "--show-current"]) == "main"


def _run(args: list[str]) -> str:
    completed = subprocess.run(  # noqa: S603
        args,
        text=True,
        capture_output=True,
        check=True,
    )
    return completed.stdout.strip()
