"""Tests for the cli_ui app module."""

from cli_ui.api import ApiClient
from cli_ui.app import (
    AgentSpaceApp,
    AgentsScreen,
    ChatScreen,
    KernelsScreen,
    SessionsScreen,
    SkillsScreen,
)


class TestAppInstantiation:
    def test_app_creates(self) -> None:
        app = AgentSpaceApp()
        assert app is not None

    def test_app_with_custom_url(self) -> None:
        app = AgentSpaceApp(base_url="http://test:9000")
        assert app is not None


class TestScreenInstantiation:
    def test_chat_screen(self) -> None:
        api = ApiClient()
        screen = ChatScreen(api)
        assert screen is not None

    def test_agents_screen(self) -> None:
        api = ApiClient()
        screen = AgentsScreen(api)
        assert screen is not None

    def test_sessions_screen(self) -> None:
        api = ApiClient()
        screen = SessionsScreen(api)
        assert screen is not None

    def test_kernels_screen(self) -> None:
        api = ApiClient()
        screen = KernelsScreen(api)
        assert screen is not None

    def test_skills_screen(self) -> None:
        api = ApiClient()
        screen = SkillsScreen(api)
        assert screen is not None
