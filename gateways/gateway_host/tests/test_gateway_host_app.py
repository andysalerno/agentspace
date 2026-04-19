from __future__ import annotations

import importlib
import sys

import pytest
from fastapi.testclient import TestClient


@pytest.fixture
def client(monkeypatch: pytest.MonkeyPatch) -> TestClient:
    monkeypatch.setenv("GATEWAY_TYPE", "echo")
    monkeypatch.setenv("GATEWAY_ID", "gw-test")
    monkeypatch.setenv("GATEWAY_AGENT_ID", "agent-test")
    monkeypatch.setenv(
        "GATEWAY_CLIENT_SERVICE_BASE_URL",
        "http://client-service.invalid",
    )
    # Force re-import so that service_from_env() picks up the new env.
    sys.modules.pop("gateway_host.app", None)
    sys.modules.pop("gateway_host.service", None)
    app_module = importlib.import_module("gateway_host.app")
    return TestClient(app_module.app)


def test_healthz(client: TestClient) -> None:
    response = client.get("/healthz")

    assert response.status_code == 200
    assert response.json() == {"status": "ok"}


def test_status_after_lifespan(client: TestClient) -> None:
    # `with` triggers lifespan startup, which calls service.start().
    with client:
        response = client.get("/status")

        assert response.status_code == 200
        body = response.json()
        assert body["type"] == "echo"
        assert body["gateway_id"] == "gw-test"
        assert body["agent_id"] == "agent-test"
        assert body["status"] == "running"


def test_logs_endpoint(client: TestClient) -> None:
    response = client.get("/logs")

    assert response.status_code == 200
    assert "lines" in response.json()
