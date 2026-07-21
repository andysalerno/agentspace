# pyright: reportPrivateUsage=false
"""Tests for ACP kernel event mapping and JSON-RPC helpers."""

from __future__ import annotations

import asyncio
import json
import sys
from typing import TYPE_CHECKING

import kernel_acp as acp_module
import pytest
from kernel.events import EventType, KernelEvent, KernelStatus
from kernel.protocol import KernelConfig
from kernel_acp import AcpKernel

if TYPE_CHECKING:
    from pathlib import Path


async def _drain(kernel: AcpKernel) -> list[KernelEvent]:
    events: list[KernelEvent] = []
    while not kernel._queue.empty():
        event = kernel._queue.get_nowait()
        if event is not None:
            events.append(event)
    return events


class TestAcpMapping:
    @pytest.fixture
    def kernel(self) -> AcpKernel:
        k = AcpKernel()
        k._session_id = "test-session"
        return k

    @pytest.mark.asyncio
    async def test_agent_message_chunk_produces_session_update(
        self,
        kernel: AcpKernel,
    ) -> None:
        await kernel._map_session_update(
            {
                "sessionId": "sess_123",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "Hello"},
                },
            },
        )

        events = await _drain(kernel)
        assert kernel._session_id == "sess_123"
        assert len(events) == 1
        assert events[0].type == EventType.SESSION_UPDATE
        assert events[0].session_id == "sess_123"
        assert events[0].method == "session/update"
        assert events[0].update == {
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "Hello"},
        }

    @pytest.mark.asyncio
    async def test_agent_thought_chunk_produces_session_update(
        self,
        kernel: AcpKernel,
    ) -> None:
        await kernel._map_session_update(
            {
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "thinking"},
                },
            },
        )

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.SESSION_UPDATE
        assert events[0].update == {
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "thinking"},
        }

    @pytest.mark.asyncio
    async def test_tool_call_and_completed_update_pass_through(
        self,
        kernel: AcpKernel,
    ) -> None:
        await kernel._map_session_update(
            {
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "call_1",
                    "title": "Run tests",
                    "status": "pending",
                    "rawInput": {"cmd": "pytest"},
                },
            },
        )
        await kernel._map_session_update(
            {
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "call_1",
                    "status": "completed",
                    "content": [
                        {
                            "type": "content",
                            "content": {"type": "text", "text": "passed"},
                        },
                    ],
                },
            },
        )

        events = await _drain(kernel)
        assert len(events) == 2
        assert events[0].type == EventType.SESSION_UPDATE
        assert events[0].update == {
            "sessionUpdate": "tool_call",
            "toolCallId": "call_1",
            "title": "Run tests",
            "status": "pending",
            "rawInput": {"cmd": "pytest"},
        }
        assert events[1].type == EventType.SESSION_UPDATE
        assert events[1].update == {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_1",
            "status": "completed",
            "content": [
                {
                    "type": "content",
                    "content": {"type": "text", "text": "passed"},
                },
            ],
        }

    @pytest.mark.asyncio
    async def test_tool_call_update_with_raw_output_passes_through(
        self,
        kernel: AcpKernel,
    ) -> None:
        await kernel._map_session_update(
            {
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "call_1",
                    "status": "completed",
                    "rawOutput": {"text": "content"},
                },
            },
        )

        events = await _drain(kernel)
        assert len(events) == 1
        assert events[0].type == EventType.SESSION_UPDATE
        assert events[0].update == {
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_1",
            "status": "completed",
            "rawOutput": {"text": "content"},
        }

    def test_build_command_defaults_to_opencode_acp(self, kernel: AcpKernel) -> None:
        assert kernel._build_command() == ["opencode", "acp"]

    def test_build_command_from_env(self, kernel: AcpKernel) -> None:
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_SERVER": "custom",
                "KERNEL_ACP_COMMAND": "my-agent --acp",
                "KERNEL_ACP_EXTRA_ARGS": "--debug\n--model=test",
            },
        )

        assert kernel._build_command() == [
            "my-agent",
            "--acp",
            "--debug",
            "--model=test",
        ]

    def test_copilot_command_is_disabled_by_default(self, kernel: AcpKernel) -> None:
        kernel._config = KernelConfig(env={"KERNEL_ACP_SERVER": "copilot"})

        with pytest.raises(ValueError, match="github/copilot-cli#4016"):
            kernel._build_command()

    def test_copilot_command_uses_acp_yolo_and_hardening(
        self,
        kernel: AcpKernel,
    ) -> None:
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_SERVER": "copilot",
                "KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED": "true",
                "KERNEL_SYSTEM_PROMPT": "be concise",
            },
        )

        assert kernel._build_command() == [
            "copilot",
            "--acp",
            "--yolo",
            "--disable-builtin-mcps",
            "--no-auto-update",
            (
                "--secret-env-vars=COPILOT_PROVIDER_API_KEY,"
                "COPILOT_PROVIDER_BEARER_TOKEN,COPILOT_PROVIDER_HEADERS"
            ),
            "--agent",
            "agentspace",
        ]

    def test_copilot_env_uses_connection_and_forces_offline(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setattr(acp_module, "COPILOT_RUNTIME_ROOT", tmp_path)
        monkeypatch.setenv("GH_TOKEN", "inherited-github-token")
        monkeypatch.setenv("COPILOT_PROVIDER_BASE_URL", "https://raw.test")
        monkeypatch.setenv("COPILOT_MODEL", "raw-model")
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_SERVER": "copilot",
                "KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED": "true",
                "CONNECTION_URL": "https://connection.test/v1",
                "CONNECTION_PROVIDER_TYPE": "anthropic",
                "CONNECTION_API_FLAVOR": "responses",
                "CONNECTION_API_KEY": "provider-secret",
                "CONNECTION_TRANSPORT": "http",
                "CONNECTION_HEADERS": "X-Tenant: example",
                "KERNEL_ACP_MODEL_NAME": "claude-model",
                "KERNEL_ACP_PROVIDER_MODEL_ID": "claude-sonnet-4",
                "KERNEL_ACP_PROVIDER_WIRE_MODEL": "claude-model-wire",
                "COPILOT_OFFLINE": "false",
            },
        )

        env = kernel._build_env()

        assert env["COPILOT_PROVIDER_BASE_URL"] == "https://connection.test/v1"
        assert env["COPILOT_PROVIDER_TYPE"] == "anthropic"
        assert env["COPILOT_PROVIDER_WIRE_API"] == "responses"
        assert env["COPILOT_PROVIDER_API_KEY"] == "provider-secret"
        assert env["COPILOT_PROVIDER_TRANSPORT"] == "http"
        assert env["COPILOT_PROVIDER_HEADERS"] == "X-Tenant: example"
        assert env["COPILOT_MODEL"] == "claude-model"
        assert env["COPILOT_PROVIDER_MODEL_ID"] == "claude-sonnet-4"
        assert env["COPILOT_PROVIDER_WIRE_MODEL"] == "claude-model-wire"
        assert env["COPILOT_OFFLINE"] == "true"
        assert env["COPILOT_HOME"] == str(tmp_path / "test-session")
        assert "GH_TOKEN" not in env
        assert "CONNECTION_API_KEY" not in env

    def test_copilot_env_maps_chat_completions_and_allows_no_key(
        self,
        kernel: AcpKernel,
    ) -> None:
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_SERVER": "copilot",
                "KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED": "true",
                "CONNECTION_URL": "http://localhost:11434/v1",
                "CONNECTION_API_FLAVOR": "chat_completions",
                "KERNEL_ACP_MODEL_NAME": "local-model",
            },
        )

        env = kernel._build_env()

        assert env["COPILOT_PROVIDER_WIRE_API"] == "completions"
        assert env["COPILOT_PROVIDER_TYPE"] == "openai"
        assert "COPILOT_PROVIDER_API_KEY" not in env

    def test_copilot_env_requires_connection_and_model(
        self,
        kernel: AcpKernel,
    ) -> None:
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_SERVER": "copilot",
                "KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED": "true",
            },
        )

        with pytest.raises(ValueError, match="CONNECTION_URL") as exc_info:
            kernel._build_env()

        assert "KERNEL_ACP_MODEL_NAME" in str(exc_info.value)

    def test_copilot_preparation_writes_agent_and_links_skills(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        runtime = tmp_path / "runtime"
        skills = tmp_path / "skills"
        skills.mkdir()
        (skills / "example").mkdir()
        monkeypatch.setattr(acp_module, "COPILOT_RUNTIME_ROOT", runtime)
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_SERVER": "copilot",
                "KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED": "true",
                "KERNEL_SYSTEM_PROMPT": "be concise",
                "KERNEL_SKILLS_DIR": str(skills),
            },
        )

        kernel._prepare_server()

        home = runtime / "test-session"
        agent = home / "agents" / "agentspace.agent.md"
        assert "be concise" in agent.read_text(encoding="utf-8")
        assert (home / "skills").is_symlink()
        assert (home / "skills").resolve() == skills.resolve()

    def test_terminal_env_excludes_provider_secrets(
        self,
        kernel: AcpKernel,
    ) -> None:
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_SERVER": "copilot",
                "KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED": "true",
                "CONNECTION_URL": "https://connection.test/v1",
                "CONNECTION_API_KEY": "provider-secret",
                "KERNEL_ACP_MODEL_NAME": "model-a",
            },
        )

        env = kernel._terminal_env(None)

        assert "COPILOT_PROVIDER_API_KEY" not in env
        assert "CONNECTION_API_KEY" not in env

        with pytest.raises(ValueError, match="provider secret"):
            kernel._terminal_env(
                [{"name": "COPILOT_PROVIDER_API_KEY", "value": "leak"}],
            )

    def test_build_env_uses_custom_default_agent_for_system_prompt(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.delenv("OPENCODE_CONFIG_CONTENT", raising=False)
        monkeypatch.setattr(
            acp_module,
            "OPENCODE_CUSTOM_AGENT_PATH",
            tmp_path / ".config" / "opencode" / "agents" / "custom.md",
        )
        kernel._config = KernelConfig(env={"KERNEL_SYSTEM_PROMPT": "be concise"})

        kernel._write_opencode_custom_agent_prompt()

        custom_agent = acp_module.OPENCODE_CUSTOM_AGENT_PATH.read_text()
        assert "mode: primary" in custom_agent
        assert "be concise" in custom_agent
        assert kernel._build_command() == ["opencode", "acp"]
        opencode_config = json.loads(kernel._build_env()["OPENCODE_CONFIG_CONTENT"])
        assert opencode_config["default_agent"] == "custom"

    def test_build_env_preserves_existing_inline_config_for_system_prompt(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setattr(
            acp_module,
            "OPENCODE_CUSTOM_AGENT_PATH",
            tmp_path / ".config" / "opencode" / "agents" / "custom.md",
        )
        kernel._config = KernelConfig(
            env={
                "KERNEL_SYSTEM_PROMPT": "be concise",
                "OPENCODE_CONFIG_CONTENT": json.dumps({"share": "disabled"}),
            },
        )

        kernel._write_opencode_custom_agent_prompt()

        opencode_config = json.loads(kernel._build_env()["OPENCODE_CONFIG_CONTENT"])
        assert opencode_config == {"share": "disabled", "default_agent": "custom"}

    def test_write_custom_agent_prompt_clears_stale_prompt(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.delenv("OPENCODE_CONFIG_CONTENT", raising=False)
        custom_agent_path = tmp_path / ".config" / "opencode" / "agents" / "custom.md"
        monkeypatch.setattr(
            acp_module,
            "OPENCODE_CUSTOM_AGENT_PATH",
            custom_agent_path,
        )
        custom_agent_path.parent.mkdir(parents=True)
        custom_agent_path.write_text("stale prompt")

        kernel._write_opencode_custom_agent_prompt()

        assert custom_agent_path.read_text() == ""
        assert kernel._build_command() == ["opencode", "acp"]
        assert "OPENCODE_CONFIG_CONTENT" not in kernel._build_env()

    def test_write_opencode_config_uses_connection_env(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setenv("HOME", str(tmp_path))
        kernel._config = KernelConfig(
            env={
                "CONNECTION_URL": "https://connection.test/v1",
                "CONNECTION_API_KEY": "from-connection",
                "KERNEL_ACP_BASE_URL": "https://legacy.test/v1",
                "KERNEL_ACP_API_KEY": "from-legacy",
                "KERNEL_ACP_MODEL_NAME": "model-a",
            },
        )

        kernel._write_opencode_config()

        config_path = tmp_path / ".config" / "opencode" / "opencode.json"
        config = json.loads(config_path.read_text())
        options = config["provider"]["customprovider"]["options"]
        assert config["$schema"] == "https://opencode.ai/config.json"
        assert (
            config["provider"]["customprovider"]["npm"] == "@ai-sdk/openai-compatible"
        )
        assert options["baseURL"] == "https://connection.test/v1"
        assert options["apiKey"] == "from-connection"
        assert config["model"] == "customprovider/model-a"
        assert config["provider"]["customprovider"]["models"] == {
            "model-a": {"name": "model-a"},
        }
        assert config["permission"]["bash"] == {"*": "allow"}
        assert config["permission"]["external_directory"] == {
            "*": "deny",
            "/tmp/**": "allow",  # noqa: S108
        }
        assert config["permission"]["webfetch"] == "deny"

    def test_write_opencode_config_uses_openai_provider_for_responses_flavor(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setenv("HOME", str(tmp_path))
        kernel._config = KernelConfig(
            env={
                "CONNECTION_URL": "https://connection.test/v1",
                "CONNECTION_API_KEY": "from-connection",
                "CONNECTION_API_FLAVOR": "responses",
                "KERNEL_ACP_MODEL_NAME": "model-a",
            },
        )

        kernel._write_opencode_config()

        config_path = tmp_path / ".config" / "opencode" / "opencode.json"
        config = json.loads(config_path.read_text())
        assert config["provider"]["customprovider"]["npm"] == "@ai-sdk/openai"

    def test_write_opencode_config_accepts_legacy_opencode_model_name(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setenv("HOME", str(tmp_path))
        kernel._config = KernelConfig(
            env={
                "CONNECTION_URL": "https://connection.test/v1",
                "CONNECTION_API_KEY": "from-connection",
                "KERNEL_OPENCODE_MODEL_NAME": "model-a",
            },
        )

        kernel._write_opencode_config()

        config_path = tmp_path / ".config" / "opencode" / "opencode.json"
        config = json.loads(config_path.read_text())
        assert config["model"] == "customprovider/model-a"
        assert config["provider"]["customprovider"]["models"] == {
            "model-a": {"name": "model-a"},
        }

    def test_write_opencode_config_preserves_unrelated_existing_config(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        monkeypatch.setenv("HOME", str(tmp_path))
        kernel._config = KernelConfig(
            env={
                "CONNECTION_URL": "https://connection.test/v1",
                "CONNECTION_API_KEY": "from-connection",
                "KERNEL_ACP_MODEL_NAME": "model-a",
            },
        )
        config_path = tmp_path / ".config" / "opencode" / "opencode.json"
        config_path.parent.mkdir(parents=True)
        config_path.write_text(
            json.dumps(
                {
                    "$schema": "https://example.test/schema.json",
                    "provider": {
                        "existing": {
                            "options": {"apiKey": "secret"},
                        },
                    },
                    "permission": {"old": "value"},
                    "theme": "dark",
                },
            ),
        )

        kernel._write_opencode_config()

        config = json.loads(config_path.read_text())
        assert config["$schema"] == "https://example.test/schema.json"
        options = config["provider"]["customprovider"]["options"]
        assert options["baseURL"] == "https://connection.test/v1"
        assert options["apiKey"] == "from-connection"
        assert config["model"] == "customprovider/model-a"
        assert config["permission"]["bash"] == {"*": "allow"}
        assert config["permission"]["question"] == "deny"
        assert config["theme"] == "dark"

    def test_write_opencode_config_reports_missing_required_env(
        self,
        kernel: AcpKernel,
    ) -> None:
        kernel._config = KernelConfig(env={"KERNEL_ACP_MODEL_NAME": "model-a"})

        with pytest.raises(ValueError, match="CONNECTION_URL") as exc_info:
            kernel._write_opencode_config()

        message = str(exc_info.value)
        assert "CONNECTION_URL" in message
        assert "CONNECTION_API_KEY" in message

    def test_permission_response_prefers_allow_once(self, kernel: AcpKernel) -> None:
        result = kernel._permission_response(
            {
                "options": [
                    {"optionId": "reject"},
                    {"optionId": "allow_once"},
                ],
            },
        )

        assert result == {
            "outcome": {"outcome": "selected", "optionId": "allow_once"},
        }

    def test_mcp_servers_from_json_env(self, kernel: AcpKernel) -> None:
        kernel._config = KernelConfig(
            env={
                "KERNEL_ACP_MCP_SERVERS": (
                    '[{"name":"fs","command":"/bin/mcp","args":[],"env":[]}]'
                ),
            },
        )

        assert kernel._mcp_servers() == [
            {"name": "fs", "command": "/bin/mcp", "args": [], "env": []},
        ]

    def test_supports_resume(self, kernel: AcpKernel) -> None:
        kernel._agent_capabilities = {"sessionCapabilities": {"resume": {}}}

        assert kernel._supports_resume() is True

    @pytest.mark.asyncio
    async def test_initialize_advertises_fs_and_terminal(
        self,
        kernel: AcpKernel,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        requests: list[tuple[str, dict[str, object]]] = []

        async def request(method: str, params: dict[str, object]) -> object:
            requests.append((method, params))
            return {"protocolVersion": 1, "agentCapabilities": {}}

        monkeypatch.setattr(kernel, "_request", request)

        await kernel._initialize()

        assert requests[0][0] == "initialize"
        assert requests[0][1]["clientCapabilities"] == {
            "fs": {"readTextFile": True, "writeTextFile": True},
            "terminal": True,
        }

    @pytest.mark.asyncio
    async def test_send_finishes_turn_without_stopping_acp_server(
        self,
        kernel: AcpKernel,
        monkeypatch: pytest.MonkeyPatch,
    ) -> None:
        class FakeProcess:
            returncode: int | None = None

        stopped = False
        kernel._process = FakeProcess()  # type: ignore[assignment]
        kernel._session_id = "sess_123"

        async def request(method: str, params: dict[str, object]) -> object:
            assert method == "session/prompt"
            assert params["sessionId"] == "sess_123"
            return {}

        async def stop_process() -> None:
            nonlocal stopped
            stopped = True

        monkeypatch.setattr(kernel, "_request", request)
        monkeypatch.setattr(kernel, "_stop_process", stop_process)

        await kernel.send("hello")

        events = await _drain(kernel)
        assert stopped is False
        assert [event.type for event in events] == [
            "session/status",
            "session/prompt/result",
            "session/status",
            "session/status",
            "session/end",
        ]
        assert events[0].status == KernelStatus.BUSY
        assert events[1].result == {}
        assert events[2].status == KernelStatus.IDLE
        assert events[3].status == KernelStatus.DONE

    def test_read_text_file_with_line_limit(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
    ) -> None:
        workspace = tmp_path
        kernel._config = KernelConfig(env={"KERNEL_ACP_WORKSPACE_DIR": str(workspace)})
        path = workspace / "src" / "example.txt"
        path.parent.mkdir()
        path.write_text("one\ntwo\nthree\n", encoding="utf-8")

        result = kernel._read_text_file(
            {
                "path": str(path),
                "line": 2,
                "limit": 1,
            },
        )

        assert result == {"content": "two\n"}

    def test_write_text_file_creates_parent_dirs(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
    ) -> None:
        workspace = tmp_path
        kernel._config = KernelConfig(env={"KERNEL_ACP_WORKSPACE_DIR": str(workspace)})
        path = workspace / "nested" / "example.txt"

        kernel._write_text_file({"path": str(path), "content": "hello"})

        assert path.read_text(encoding="utf-8") == "hello"

    def test_filesystem_rejects_paths_outside_workspace(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
    ) -> None:
        workspace = tmp_path
        kernel._config = KernelConfig(env={"KERNEL_ACP_WORKSPACE_DIR": str(workspace)})
        outside = workspace.parent / "outside.txt"

        with pytest.raises(ValueError, match="outside workspace"):
            kernel._write_text_file({"path": str(outside), "content": "nope"})

    @pytest.mark.asyncio
    async def test_terminal_create_output_wait_and_release(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
    ) -> None:
        workspace = tmp_path
        kernel._config = KernelConfig(env={"KERNEL_ACP_WORKSPACE_DIR": str(workspace)})

        created = await kernel._terminal_create(
            {
                "command": sys.executable,
                "args": ["-c", "print('hello from terminal')"],
                "cwd": str(workspace),
            },
        )
        terminal_id = created["terminalId"]
        assert isinstance(terminal_id, str)

        exit_status = await kernel._terminal_wait_for_exit(
            {"terminalId": terminal_id},
        )
        output = kernel._terminal_output({"terminalId": terminal_id})
        await kernel._terminal_release({"terminalId": terminal_id})

        assert exit_status == {"exitCode": 0, "signal": None}
        assert output["truncated"] is False
        assert output["exitStatus"] == {"exitCode": 0, "signal": None}
        assert "hello from terminal" in str(output["output"])

    @pytest.mark.asyncio
    async def test_terminal_kill(
        self,
        kernel: AcpKernel,
        tmp_path: Path,
    ) -> None:
        workspace = tmp_path
        kernel._config = KernelConfig(env={"KERNEL_ACP_WORKSPACE_DIR": str(workspace)})

        created = await kernel._terminal_create(
            {
                "command": sys.executable,
                "args": ["-c", "import time; time.sleep(30)"],
                "cwd": str(workspace),
            },
        )
        terminal_id = str(created["terminalId"])
        await asyncio.sleep(0.05)

        await kernel._terminal_kill({"terminalId": terminal_id})
        output = kernel._terminal_output({"terminalId": terminal_id})
        await kernel._terminal_release({"terminalId": terminal_id})

        exit_status = output["exitStatus"]
        assert isinstance(exit_status, dict)
        assert exit_status["exitCode"] is None or isinstance(
            exit_status["exitCode"],
            int,
        )
