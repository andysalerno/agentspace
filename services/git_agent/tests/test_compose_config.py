from __future__ import annotations

from pathlib import Path


def test_git_agent_data_uses_stable_named_volume() -> None:
    compose = Path(__file__).resolve().parents[3].joinpath("compose.yaml").read_text()

    assert "- git-agent-data:/data" in compose
    assert "git-agent-data:\n" in compose
    assert "name: ${GITAGENT_DATA_VOLUME:-agentspace-git-agent-data}" in compose
    assert (
        "CLIENT_SERVICE_GIT_AGENT_DATA_VOLUME="
        "${GITAGENT_DATA_VOLUME:-agentspace-git-agent-data}"
    ) in compose


def test_git_agent_remote_url_matches_repo_path() -> None:
    compose = Path(__file__).resolve().parents[3].joinpath("compose.yaml").read_text()

    assert (
        "AGENT_HOST_GITAGENT_REMOTE_URL="
        "${AGENT_HOST_GITAGENT_REMOTE_URL:-http://gitagent:8004/repo.git}"
    ) in compose
    assert "GITAGENT_REPO_PATH=${GITAGENT_REPO_PATH:-/data/repos/repo.git}" in compose
    assert "GITAGENT_REPO_PATH=/data/repos/main.git" not in compose
