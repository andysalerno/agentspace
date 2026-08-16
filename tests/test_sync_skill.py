from __future__ import annotations

import runpy
from pathlib import Path
from typing import Any, Protocol, cast

import pytest

SCRIPT = (
    Path(__file__).parents[1]
    / "mounts"
    / "skills"
    / "manage-skills"
    / "scripts"
    / "sync_skill.py"
)
SCRIPT_GLOBALS = runpy.run_path(str(SCRIPT), run_name="sync_skill")


class CollectFiles(Protocol):
    def __call__(self, skill_dir: Path) -> dict[str, str]: ...


class RequestJson(Protocol):
    def __call__(
        self,
        method: str,
        url: str,
        payload: dict[str, Any] | None = None,
    ) -> tuple[int, dict[str, Any]]: ...


class SyncSkill(Protocol):
    def __call__(
        self,
        skill_dir: Path,
        api_url: str,
        agent_id: str | None,
        request: RequestJson,
    ) -> dict[str, Any]: ...


collect_files = cast("CollectFiles", SCRIPT_GLOBALS["collect_files"])
sync_skill = cast("SyncSkill", SCRIPT_GLOBALS["sync_skill"])


def make_skill(tmp_path: Path) -> Path:
    skill_dir = tmp_path / "weather-report"
    (skill_dir / "scripts").mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Weather\n", encoding="utf-8")
    (skill_dir / "scripts" / "forecast.py").write_text(
        'print("sunny")\n',
        encoding="utf-8",
    )
    return skill_dir


def test_collect_files_recursively_reads_multifile_skill(tmp_path: Path) -> None:
    assert collect_files(make_skill(tmp_path)) == {
        "SKILL.md": "# Weather\n",
        "scripts/forecast.py": 'print("sunny")\n',
    }


def test_sync_skill_creates_missing_skill_and_attributes_creator(
    tmp_path: Path,
) -> None:
    requests: list[tuple[str, str, dict[str, Any] | None]] = []

    def request(
        method: str,
        url: str,
        payload: dict[str, Any] | None = None,
    ) -> tuple[int, dict[str, Any]]:
        requests.append((method, url, payload))
        if method == "GET":
            return 404, {}
        return 200, {"skill_id": "weather-report"}

    result = sync_skill(
        make_skill(tmp_path),
        "http://client-service:8002/skills/",
        "weather-agent",
        request,
    )

    assert result == {"skill_id": "weather-report"}
    assert requests == [
        ("GET", "http://client-service:8002/skills/weather-report", None),
        (
            "POST",
            "http://client-service:8002/skills",
            {
                "skill_id": "weather-report",
                "creator_agent_id": "weather-agent",
                "files": {
                    "SKILL.md": "# Weather\n",
                    "scripts/forecast.py": 'print("sunny")\n',
                },
            },
        ),
    ]


def test_sync_skill_updates_user_skill(tmp_path: Path) -> None:
    methods: list[str] = []

    def request(
        method: str,
        url: str,
        payload: dict[str, Any] | None = None,
    ) -> tuple[int, dict[str, Any]]:
        _ = url, payload
        methods.append(method)
        if method == "GET":
            return 200, {"source": "user"}
        return 200, {"version": 2}

    assert sync_skill(make_skill(tmp_path), "http://skills", None, request) == {
        "version": 2,
    }
    assert methods == ["GET", "PUT"]


def test_sync_skill_refuses_to_update_builtin(tmp_path: Path) -> None:
    def request(
        method: str,
        url: str,
        payload: dict[str, Any] | None = None,
    ) -> tuple[int, dict[str, Any]]:
        _ = method, url, payload
        return 200, {"source": "builtin"}

    with pytest.raises(ValueError, match="refusing to update non-user skill"):
        sync_skill(make_skill(tmp_path), "http://skills", None, request)
