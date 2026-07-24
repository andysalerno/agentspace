from __future__ import annotations

import json

import httpx
from cli_channel.client import (
    ClientServiceSessionClient,
    SessionRegistration,
    SessionReply,
)


def test_client_dataclasses_expose_expected_fields() -> None:
    registration = SessionRegistration(
        session_id="session-1",
        agent_id="agent-one",
        channel_name="terminal-1",
    )
    reply = SessionReply(session_id="session-1", assistant_text="hello")

    assert registration.session_id == "session-1"
    assert registration.channel_name == "terminal-1"
    assert reply.assistant_text == "hello"


async def test_config_apply_posts_yaml_bytes() -> None:
    source = b"apiVersion: agentspace.dev/v1alpha1\nkind: AgentSpaceConfig\n"

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.method == "POST"
        assert request.url.path == "/config/apply"
        assert request.headers["content-type"] == "application/yaml"
        assert request.content == source
        return httpx.Response(200, json={"generation": 2})

    client = ClientServiceSessionClient(
        base_url="http://test",
        transport=httpx.MockTransport(handler),
    )
    response = await client.apply_config(source)

    assert response == {"generation": 2}


async def test_config_export_preserves_content_and_filename() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/config/export"
        assert request.url.params["mode"] == "canonical"
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
    download = await client.export_config("canonical")

    assert download.content == b"kind: AgentSpaceConfig\n"
    assert download.filename == "agentspace.yaml"
    assert download.content_type == "application/yaml"


async def test_secret_values_are_write_only() -> None:
    requests: list[tuple[str, str, bytes]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append((request.method, request.url.path, request.content))
        if request.method == "GET":
            return httpx.Response(
                200,
                json=[
                    {
                        "name": "TOKEN",
                        "description": "Service token",
                        "is_set": True,
                        "references": ["connections/primary/apiKey"],
                    },
                ],
            )
        return httpx.Response(204)

    client = ClientServiceSessionClient(
        base_url="http://test",
        transport=httpx.MockTransport(handler),
    )

    secrets = await client.list_secrets()
    await client.set_secret_value("TOKEN", "super-secret")
    await client.clear_secret_value("TOKEN")

    assert secrets[0].name == "TOKEN"
    assert secrets[0].is_set
    assert secrets[0].references == ("connections/primary/apiKey",)
    assert requests[1][:2] == ("PUT", "/secrets/TOKEN/value")
    assert json.loads(requests[1][2]) == {"value": "super-secret"}
    assert requests[2][:2] == ("DELETE", "/secrets/TOKEN/value")
