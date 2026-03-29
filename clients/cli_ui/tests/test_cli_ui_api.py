"""Tests for the cli_ui API client."""

from cli_ui.api import ApiClient


class TestApiClient:
    def test_default_base_url(self) -> None:
        client = ApiClient()
        assert client.base_url == "http://127.0.0.1:8002"

    def test_custom_base_url(self) -> None:
        client = ApiClient(base_url="http://example.com:9000")
        assert client.base_url == "http://example.com:9000"

    def test_default_timeout(self) -> None:
        client = ApiClient()
        assert client.timeout == 120.0
