from __future__ import annotations

import shutil
import subprocess
import uuid
from pathlib import Path
from typing import TYPE_CHECKING, Literal

import pytest
from fastapi.testclient import TestClient

from git_agent import Settings, create_app
from git_agent.patch_parser import EMPTY_TREE_SHA, NULL_SHA, ChangedLine, analyze_patch
from git_agent.reviewer import ReviewContext, Reviewer, ReviewerRawResponse

if TYPE_CHECKING:
    from collections.abc import Iterator, Sequence

WORKSPACE_ROOT = Path(__file__).resolve().parents[1] / ".test-workspaces"

ADD_README_PATCH = """diff --git a/README.md b/README.md
new file mode 100644
index 0000000..ce01362
--- /dev/null
+++ b/README.md
@@ -0,0 +1 @@
+hello
"""

ADD_OTHER_PATCH = """diff --git a/OTHER.md b/OTHER.md
new file mode 100644
index 0000000..b6fc4c6
--- /dev/null
+++ b/OTHER.md
@@ -0,0 +1 @@
+other
"""

CONFLICTING_README_PATCH = """diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-goodbye
+conflict
"""

UNSAFE_PATH_PATCH = """diff --git a/../evil b/../evil
new file mode 100644
--- /dev/null
+++ b/../evil
@@ -0,0 +1 @@
+bad
"""

LINE_PATCH = """diff --git a/app.py b/app.py
--- a/app.py
+++ b/app.py
@@ -10,2 +10,2 @@
-old
+new
 keep
"""


class BadLineReviewer:
    async def review(self, context: ReviewContext) -> ReviewerRawResponse:
        return ReviewerRawResponse(
            payload={
                "accepted": False,
                "summary": "bad line",
                "comments": [
                    {
                        "path": "README.md",
                        "side": "right",
                        "line": 99,
                        "message": f"not a changed line for {context.request_id}",
                    },
                ],
            },
        )


class SequenceReviewer:
    def __init__(self, decisions: Sequence[bool]) -> None:
        self._decisions = list(decisions)

    async def review(self, context: ReviewContext) -> ReviewerRawResponse:
        accepted = self._decisions.pop(0)
        return ReviewerRawResponse(
            payload={
                "accepted": accepted,
                "summary": f"sequence decision for {context.request_id}",
                "comments": []
                if accepted
                else [{"side": "general", "message": "sequence rejected"}],
            },
            session_id=f"review-{len(self._decisions)}",
        )


@pytest.fixture
def workspace() -> Iterator[Path]:
    path = WORKSPACE_ROOT / f"case-{uuid.uuid4().hex}"
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    try:
        yield path
    finally:
        if path.exists():
            shutil.rmtree(path)


def make_client(
    workspace: Path,
    *,
    review_mode: Literal["auto_accept", "auto_reject", "invalid"] = "auto_accept",
    reviewer: Reviewer | None = None,
) -> TestClient:
    settings = Settings(
        repo_path=workspace / "repo.git",
        db_path=workspace / "requests.sqlite3",
        scratch_path=workspace / "worktrees",
        review_mode=review_mode,
    )
    return TestClient(create_app(settings=settings, reviewer=reviewer))


def test_empty_repo_init_status_and_receive_pack_denied(workspace: Path) -> None:
    with make_client(workspace) as client:
        assert client.get("/healthz").json() == {"status": "ok"}
        status = client.get("/status").json()
        assert status["empty"] is True
        assert status["repo_name"] == "repo.git"
        assert status["refs"] == []

        denied = client.get("/repo.git/info/refs?service=git-receive-pack")
        assert denied.status_code == 403
        assert "receive-pack is disabled" in denied.text

        upload_pack = client.get("/repo.git/info/refs?service=git-upload-pack")
        assert upload_pack.status_code == 200
        assert "git-upload-pack" in upload_pack.text


def test_wip_patch_accepts_and_persists(workspace: Path) -> None:
    with make_client(workspace) as client:
        response = client.post(
            "/PatchRequest",
            json={
                "target_ref": "wip/demo",
                "base_sha": EMPTY_TREE_SHA,
                "raw_patch": ADD_README_PATCH,
                "commit_message": "Add README",
                "author": {"name": "Agent", "email": "agent@example.invalid"},
                "argument": {"reason": "wip branch"},
                "response_to_request_id": "previous-request",
            },
        )
        body = response.json()
        assert body["status"] == "accepted"
        assert body["accepted"] is True
        assert body["request_id"] == body["id"]
        assert body["response_to_request_id"] == "previous-request"
        assert body["target_ref"] == "refs/heads/wip/demo"
        assert isinstance(body["commit_sha"], str)

        status = client.get("/status").json()
        refs = {item["ref"]: item["sha"] for item in status["refs"]}
        assert refs["refs/heads/wip/demo"] == body["commit_sha"]

        listed = client.get("/patch-requests").json()["patch_requests"]
        assert listed[0]["id"] == body["id"]
        assert "raw_patch" not in listed[0]

        detail = client.get(f"/patch-requests/{body['id']}").json()
        assert detail["raw_patch"] == ADD_README_PATCH
        assert client.get(f"/patch-requests/{body['id']}/raw").text == ADD_README_PATCH


def test_protected_main_stale_and_conflict(workspace: Path) -> None:
    with make_client(workspace) as client:
        first = client.post(
            "/patch-requests",
            json={
                "target_ref": "main",
                "base_sha": EMPTY_TREE_SHA,
                "patch": ADD_README_PATCH,
                "commit_message": "Initialize main",
            },
        ).json()
        assert first["status"] == "accepted"
        current_head = first["commit_sha"]

        stale = client.post(
            "/PatchRequest",
            json={
                "target_ref": "refs/heads/main",
                "base_sha": NULL_SHA,
                "raw_patch": ADD_OTHER_PATCH,
                "commit_message": "Stale change",
            },
        ).json()
        assert stale["status"] == "stale_base"
        assert stale["accepted"] is False
        assert "Fetch the latest refs" in stale["comments"][0]["message"]

        conflict = client.post(
            "/PatchRequest",
            json={
                "target_ref": "main",
                "base_sha": current_head,
                "raw_patch": CONFLICTING_README_PATCH,
                "commit_message": "Conflicting change",
            },
        ).json()
        assert conflict["status"] == "conflict"
        assert conflict["accepted"] is False
        assert "does not apply cleanly" in conflict["comments"][0]["message"]


def test_line_indexes_and_unsafe_path_rejection(workspace: Path) -> None:
    analysis = analyze_patch(LINE_PATCH)
    assert ChangedLine("app.py", "left", 10) in analysis.changed_lines
    assert ChangedLine("app.py", "right", 10) in analysis.changed_lines
    assert analysis.line_indexes_json() == [
        {"path": "app.py", "side": "left", "line": 10},
        {"path": "app.py", "side": "right", "line": 10},
    ]

    with make_client(workspace) as client:
        rejected = client.post(
            "/PatchRequest",
            json={
                "target_ref": "wip/unsafe",
                "raw_patch": UNSAFE_PATH_PATCH,
                "commit_message": "Unsafe path",
            },
        ).json()
        assert rejected["status"] == "rejected"
        assert rejected["accepted"] is False
        assert rejected["comments"][0]["code"] == "invalid_request"


def test_binary_patch_handling_for_wip(workspace: Path) -> None:
    patch = _binary_patch(workspace / "binary-source")
    assert "GIT binary patch" in patch
    with make_client(workspace) as client:
        accepted = client.post(
            "/PatchRequest",
            json={
                "target_ref": "wip/binary",
                "raw_patch": patch,
                "commit_message": "Add binary asset",
            },
        ).json()
        assert accepted["status"] == "accepted"
        assert accepted["accepted"] is True
        assert accepted["binary_paths"] == ["asset.bin"]


def test_reviewer_invalid_response_fails_closed(workspace: Path) -> None:
    with make_client(workspace, review_mode="invalid") as client:
        response = client.post(
            "/PatchRequest",
            json={
                "target_ref": "main",
                "base_sha": NULL_SHA,
                "raw_patch": ADD_README_PATCH,
                "commit_message": "Should not land",
            },
        ).json()
        assert response["status"] == "review_error"
        assert response["accepted"] is False
        assert response["comments"][0]["code"] == "review_error"
        assert client.get("/status").json()["refs"] == []


def test_reviewer_comments_must_map_to_changed_lines(workspace: Path) -> None:
    with make_client(workspace, reviewer=BadLineReviewer()) as client:
        response = client.post(
            "/PatchRequest",
            json={
                "target_ref": "main",
                "base_sha": NULL_SHA,
                "raw_patch": ADD_README_PATCH,
                "commit_message": "Should not land",
            },
        ).json()
        assert response["status"] == "review_error"
        assert response["accepted"] is False
        assert client.get("/status").json()["refs"] == []


def test_rerun_review_reprocesses_unaccepted_protected_request(
    workspace: Path,
) -> None:
    with make_client(workspace, reviewer=SequenceReviewer([False, True])) as client:
        rejected = client.post(
            "/PatchRequest",
            json={
                "target_ref": "main",
                "base_sha": NULL_SHA,
                "raw_patch": ADD_README_PATCH,
                "commit_message": "Retry review",
            },
        ).json()
        assert rejected["status"] == "denied"
        assert client.get("/status").json()["refs"] == []

        rerun = client.post(
            f"/patch-requests/{rejected['id']}/rerun-review",
        ).json()
        assert rerun["id"] == rejected["id"]
        assert rerun["status"] == "accepted"
        assert rerun["accepted"] is True
        assert isinstance(rerun["commit_sha"], str)
        assert rerun["reviewer_session_id"] == "review-0"

        status = client.get("/status").json()
        refs = {item["ref"]: item["sha"] for item in status["refs"]}
        assert refs["refs/heads/main"] == rerun["commit_sha"]

        second_rerun = client.post(f"/patch-requests/{rejected['id']}/rerun-review")
        assert second_rerun.status_code == 409


def test_rerun_review_rejects_wip_requests(workspace: Path) -> None:
    with make_client(workspace) as client:
        accepted = client.post(
            "/PatchRequest",
            json={
                "target_ref": "wip/no-review",
                "raw_patch": ADD_README_PATCH,
                "commit_message": "WIP skips review",
            },
        ).json()

        rerun = client.post(f"/patch-requests/{accepted['id']}/rerun-review")
        assert rerun.status_code == 409


def _binary_patch(repo_path: Path) -> str:
    repo_path.mkdir(parents=True)
    _run(["git", "init", str(repo_path)])
    _run(["git", "-C", str(repo_path), "config", "user.name", "Tester"])
    _run(
        ["git", "-C", str(repo_path), "config", "user.email", "tester@example.invalid"],
    )
    _run(["git", "-C", str(repo_path), "commit", "--allow-empty", "-m", "base"])
    (repo_path / "asset.bin").write_bytes(bytes(range(256)))
    _run(["git", "-C", str(repo_path), "add", "asset.bin"])
    return _run(["git", "-C", str(repo_path), "diff", "--binary", "--cached"])


def _run(args: Sequence[str]) -> str:
    completed = subprocess.run(  # noqa: S603
        list(args),
        text=True,
        capture_output=True,
        check=True,
    )
    return completed.stdout
