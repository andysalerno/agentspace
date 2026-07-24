from __future__ import annotations

import io
import json
import sys
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
