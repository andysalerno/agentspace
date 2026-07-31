from __future__ import annotations

import io
import json
import sys
import zipfile
from typing import TYPE_CHECKING

import httpx
from cli_channel.__main__ import parse_args, run
from cli_channel.client import ClientServiceSessionClient

if TYPE_CHECKING:
    from pathlib import Path

    import pytest


def test_parse_config_export_resource() -> None:
    args = parse_args(
        [
            "config",
            "export",
            "agent/researcher",
            "--mode",
            "canonical",
        ],
    )

    assert args.command == "config"
    assert args.config_command == "export"
    assert args.resource == "agent/researcher"
    assert args.mode == "canonical"


async def test_secret_set_reads_value_from_stdin(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.method == "PUT"
        assert request.url.path == "/secrets/TOKEN/value"
        assert json.loads(request.content) == {"value": "hidden-value"}
        return httpx.Response(204)

    client = ClientServiceSessionClient(
        base_url="http://test",
        transport=httpx.MockTransport(handler),
    )
    monkeypatch.setattr(sys, "stdin", io.StringIO("hidden-value\n"))

    await run(["secret", "set", "TOKEN", "--value-stdin"], client)

    output = capsys.readouterr().out
    assert output == "TOKEN: set\n"
    assert "hidden-value" not in output


async def test_config_export_writes_server_filename(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/config/export"
        return httpx.Response(
            200,
            content=b"kind: AgentSpaceConfig\n",
            headers={
                "content-type": "application/yaml",
                "content-disposition": 'attachment; filename="agentspace.yaml"',
            },
        )

    client = ClientServiceSessionClient(
        base_url="http://test",
        transport=httpx.MockTransport(handler),
    )
    monkeypatch.chdir(tmp_path)

    await run(["config", "export"], client)

    assert (tmp_path / "agentspace.yaml").read_bytes() == b"kind: AgentSpaceConfig\n"
    assert capsys.readouterr().out == "agentspace.yaml\n"


async def test_config_directory_uploads_deterministic_zip(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    (tmp_path / "agentspace.yaml").write_text(
        "apiVersion: agentspace.dev/v1alpha1\nkind: AgentSpaceConfig\n",
    )
    skill_dir = tmp_path / "skills" / "research"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text("# Research\n")

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/config/validate"
        assert request.headers["content-type"] == "application/zip"
        with zipfile.ZipFile(io.BytesIO(request.content)) as archive:
            assert archive.namelist() == [
                "agentspace.yaml",
                "skills/research/SKILL.md",
            ]
            assert archive.read("skills/research/SKILL.md") == b"# Research\n"
        return httpx.Response(200, json={"valid": True})

    client = ClientServiceSessionClient(
        base_url="http://test",
        transport=httpx.MockTransport(handler),
    )

    await run(["config", "validate", "-f", str(tmp_path)], client)

    assert '"valid": true' in capsys.readouterr().out
