from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from copilot_launch import (
    CopilotLaunchConfig,
    CopilotLaunchError,
    agent_profile_name,
    build_chat_launch,
    build_copilot_environment,
    build_interactive_launch,
    prepare_workspace_artifacts,
)

if TYPE_CHECKING:
    from pathlib import Path

SESSION_ID = "0cb916db-26aa-40f2-86b5-1ba81b225fd2"
OTHER_SESSION_ID = "ad2a4229-152a-4ba7-b1e9-528290881af4"


def test_chat_launch_preserves_runtime_options(tmp_path: Path) -> None:
    config = CopilotLaunchConfig(
        session_id=SESSION_ID,
        workspace_dir=str(tmp_path),
        additional_paths=("/repo",),
        env={
            "COPILOT_MODEL": "gpt-5.2",
            "COPILOT_REASONING_EFFORT": "high",
            "COPILOT_CONFIG_DIR": "/root/.copilot",
            "COPILOT_ADDITIONAL_PATHS": "/workspace:/workspace-extra",
            "COPILOT_EXTRA_ARGS": "--enable-all-github-mcp-tools\n--stream\non",
        },
    )

    launch = build_chat_launch(config, "hello", process_env={})

    assert launch.argv[:4] == ("copilot", "-p", "hello", "--output-format")
    assert f"--session-id={SESSION_ID}" in launch.argv
    assert not any(arg.startswith("--resume") for arg in launch.argv)
    assert launch.argv[
        launch.argv.index("--model") : launch.argv.index("--model") + 2
    ] == ("--model", "gpt-5.2")
    assert launch.argv.count("--add-dir") == 3
    assert "/repo" in launch.argv
    assert "/workspace" in launch.argv
    assert "/workspace-extra" in launch.argv
    assert "--enable-all-github-mcp-tools" in launch.argv
    assert launch.redacted_argv[2] == "<prompt redacted: 5 chars>"


@pytest.mark.parametrize(
    ("api_flavor", "wire_api"),
    [("chat_completions", "completions"), ("responses", "responses")],
)
def test_connection_translates_to_copilot_provider(
    api_flavor: str,
    wire_api: str,
) -> None:
    environment = build_copilot_environment(
        {
            "CONNECTION_URL": "https://provider.example/v1",
            "CONNECTION_API_KEY": "selected-key",
            "CONNECTION_API_FLAVOR": api_flavor,
            "COPILOT_MODEL": "model-a",
        },
        process_env={
            "COPILOT_PROVIDER_TYPE": "anthropic",
            "COPILOT_PROVIDER_BASE_URL": "https://stale.example",
            "COPILOT_PROVIDER_API_KEY": "stale-key",
            "COPILOT_PROVIDER_BEARER_TOKEN": "stale-bearer",
            "COPILOT_PROVIDER_WIRE_API": "stale",
        },
    )

    assert environment["COPILOT_PROVIDER_TYPE"] == "openai"
    assert environment["COPILOT_PROVIDER_BASE_URL"] == "https://provider.example/v1"
    assert environment["COPILOT_PROVIDER_API_KEY"] == "selected-key"
    assert environment["COPILOT_PROVIDER_WIRE_API"] == wire_api
    assert environment["COPILOT_MODEL"] == "model-a"
    assert "COPILOT_PROVIDER_BEARER_TOKEN" not in environment
    assert "CONNECTION_URL" not in environment
    assert "CONNECTION_API_KEY" not in environment
    assert "CONNECTION_API_FLAVOR" not in environment


def test_no_connection_preserves_github_auth_and_clears_provider() -> None:
    environment = build_copilot_environment(
        {},
        process_env={
            "GH_TOKEN": "github-token",
            "COPILOT_PROVIDER_API_KEY": "stale",
            "COPILOT_PROVIDER_BEARER_TOKEN": "stale-bearer",
        },
    )

    assert environment["GH_TOKEN"] == "github-token"  # noqa: S105
    assert "COPILOT_PROVIDER_API_KEY" not in environment
    assert "COPILOT_PROVIDER_BEARER_TOKEN" not in environment


def test_incomplete_connection_is_rejected() -> None:
    with pytest.raises(CopilotLaunchError, match="CONNECTION_URL"):
        build_copilot_environment(
            {"CONNECTION_API_KEY": "key", "CONNECTION_API_FLAVOR": "responses"},
            process_env={},
        )


def test_secret_env_flag_covers_provider_and_generic_credentials(
    tmp_path: Path,
) -> None:
    launch = build_chat_launch(
        CopilotLaunchConfig(session_id=SESSION_ID, env={}, workspace_dir=str(tmp_path)),
        "hello",
        process_env={},
    )

    assert (
        "--secret-env-vars=COPILOT_PROVIDER_API_KEY,"
        "COPILOT_PROVIDER_BEARER_TOKEN,CONNECTION_API_KEY"
    ) in launch.argv


@pytest.mark.parametrize("extra_arg", ["--resume=old", "--session-id", "-r"])
def test_extra_args_cannot_override_session_identity(
    tmp_path: Path,
    extra_arg: str,
) -> None:
    config = CopilotLaunchConfig(
        session_id=SESSION_ID,
        workspace_dir=str(tmp_path),
        env={"COPILOT_EXTRA_ARGS": extra_arg},
    )

    with pytest.raises(CopilotLaunchError, match="session identity"):
        build_chat_launch(config, "hello", process_env={})


def test_session_id_must_be_uuid(tmp_path: Path) -> None:
    config = CopilotLaunchConfig(
        session_id="not-a-uuid",
        workspace_dir=str(tmp_path),
        env={},
    )

    with pytest.raises(CopilotLaunchError, match="must be a UUID"):
        build_interactive_launch(config, process_env={})


def test_interactive_launch_uses_shared_session_and_provider_semantics(
    tmp_path: Path,
) -> None:
    launch = build_interactive_launch(
        CopilotLaunchConfig(
            session_id=SESSION_ID,
            workspace_dir=str(tmp_path),
            env={
                "AGENTSPACE_SESSION_ID": OTHER_SESSION_ID,
                "CONNECTION_URL": "https://provider.example/v1",
                "CONNECTION_API_FLAVOR": "responses",
                "KERNEL_SYSTEM_PROMPT": "interactive prompt",
            },
        ),
        process_env={},
    )

    assert launch.argv[:4] == (
        "copilot",
        "--allow-all",
        "--no-auto-update",
        "--mouse=on",
    )
    assert f"--session-id={SESSION_ID}" in launch.argv
    assert f"--agent=agentspace-{OTHER_SESSION_ID}" in launch.argv
    assert "-p" not in launch.argv
    assert launch.environment["COPILOT_PROVIDER_API_KEY"] == "not-required"
    assert launch.environment["COPILOT_PROVIDER_WIRE_API"] == "responses"


def test_interactive_launch_owns_metadata_only_telemetry_environment(
    tmp_path: Path,
) -> None:
    launch_id = "4cb3df39-797e-4542-8fd1-d24665699e4d"
    launch = build_interactive_launch(
        CopilotLaunchConfig(
            session_id=SESSION_ID,
            workspace_dir=str(tmp_path),
            env={"AGENTSPACE_SESSION_ID": OTHER_SESSION_ID},
        ),
        process_env={
            "COPILOT_OTEL_ENABLED": "false",
            "COPILOT_OTEL_EXPORTER_TYPE": "otlp-http",
            "OTEL_EXPORTER_OTLP_ENDPOINT": "https://elsewhere.example",
            "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT": "true",
        },
        telemetry_file_path=f"/var/lib/agentspace/telemetry/{launch_id}.jsonl",
    )

    assert launch.environment["COPILOT_OTEL_ENABLED"] == "true"
    assert launch.environment["COPILOT_OTEL_EXPORTER_TYPE"] == "file"
    assert launch.environment["COPILOT_OTEL_FILE_EXPORTER_PATH"].endswith(
        f"/{launch_id}.jsonl",
    )
    assert (
        launch.environment["OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"]
        == "false"
    )
    assert (
        launch.environment["OTEL_RESOURCE_ATTRIBUTES"]
        == f"agentspace.session.id={OTHER_SESSION_ID}"
    )
    assert "OTEL_EXPORTER_OTLP_ENDPOINT" not in launch.environment


def test_chat_launch_scrubs_telemetry_environment(tmp_path: Path) -> None:
    launch = build_chat_launch(
        CopilotLaunchConfig(
            session_id=SESSION_ID,
            workspace_dir=str(tmp_path),
            env={"COPILOT_OTEL_ENABLED": "true"},
        ),
        "hello",
        process_env={
            "COPILOT_OTEL_EXPORTER_TYPE": "otlp-http",
            "OTEL_EXPORTER_OTLP_ENDPOINT": "https://elsewhere.example",
        },
    )

    assert not any(
        name.startswith(("OTEL_", "COPILOT_OTEL_")) for name in launch.environment
    )


@pytest.mark.parametrize(
    "path",
    [
        "/unmanaged/launch.jsonl",
        "/var/lib/agentspace/telemetry/not-a-uuid.jsonl",
        "/var/lib/agentspace/telemetry/4cb3df39-797e-4542-8fd1-d24665699e4d.log",
    ],
)
def test_interactive_launch_rejects_unmanaged_telemetry_path(
    tmp_path: Path,
    path: str,
) -> None:
    with pytest.raises(CopilotLaunchError, match="telemetry"):
        build_interactive_launch(
            CopilotLaunchConfig(
                session_id=SESSION_ID,
                workspace_dir=str(tmp_path),
                env={"AGENTSPACE_SESSION_ID": OTHER_SESSION_ID},
            ),
            process_env={},
            telemetry_file_path=path,
        )


@pytest.mark.parametrize(
    "path",
    [
        "/var/lib/agentspace/telemetry/4CB3DF39-797E-4542-8FD1-D24665699E4D.jsonl",
        "/var/lib/agentspace/telemetry/4cb3df39797e45428fd1d24665699e4d.jsonl",
    ],
)
def test_interactive_launch_rejects_noncanonical_uuid_telemetry_path(
    tmp_path: Path,
    path: str,
) -> None:
    with pytest.raises(CopilotLaunchError, match="canonical UUID"):
        build_interactive_launch(
            CopilotLaunchConfig(
                session_id=SESSION_ID,
                workspace_dir=str(tmp_path),
                env={"AGENTSPACE_SESSION_ID": OTHER_SESSION_ID},
            ),
            process_env={},
            telemetry_file_path=path,
        )


def test_profile_is_deterministic_owned_and_selected(tmp_path: Path) -> None:
    session_id = SESSION_ID
    config = CopilotLaunchConfig(
        session_id=session_id,
        workspace_dir=str(tmp_path),
        env={"KERNEL_SYSTEM_PROMPT": "Follow the AgentSpace instructions."},
    )

    launch = build_chat_launch(config, "hello", process_env={})
    expected_name = f"agentspace-{session_id}"
    profile = tmp_path / ".github/agents" / f"{expected_name}.agent.md"

    assert agent_profile_name(session_id) == expected_name
    assert profile.is_file()
    assert 'description: "AgentSpace session profile"' in profile.read_text()
    assert "Follow the AgentSpace instructions." in profile.read_text()
    assert f"--agent={expected_name}" in launch.argv
    assert launch.artifacts.owned_relative_paths == (
        f".github/agents/{expected_name}.agent.md",
    )


def test_empty_prompt_removes_only_owned_profile(tmp_path: Path) -> None:
    session_id = SESSION_ID
    name = agent_profile_name(session_id)
    profile = tmp_path / ".github/agents" / f"{name}.agent.md"
    profile.parent.mkdir(parents=True)
    profile.write_text("user content", encoding="utf-8")

    artifacts = prepare_workspace_artifacts(
        CopilotLaunchConfig(session_id=session_id, workspace_dir=str(tmp_path), env={}),
    )
    assert profile.read_text(encoding="utf-8") == "user content"
    assert artifacts.agent_name is None

    profile.write_text(
        f"---\ndescription: owned\n---\n<!-- agentspace-owned-profile:{name} -->\n",
        encoding="utf-8",
    )
    prepare_workspace_artifacts(
        CopilotLaunchConfig(session_id=session_id, workspace_dir=str(tmp_path), env={}),
    )
    assert not profile.exists()


def test_nonempty_prompt_does_not_replace_user_profile(tmp_path: Path) -> None:
    name = agent_profile_name(SESSION_ID)
    profile = tmp_path / ".github/agents" / f"{name}.agent.md"
    profile.parent.mkdir(parents=True)
    profile.write_text("user content", encoding="utf-8")

    with pytest.raises(CopilotLaunchError, match="user-authored"):
        prepare_workspace_artifacts(
            CopilotLaunchConfig(
                session_id=SESSION_ID,
                workspace_dir=str(tmp_path),
                env={"KERNEL_SYSTEM_PROMPT": "managed prompt"},
            ),
        )

    assert profile.read_text(encoding="utf-8") == "user content"


def test_artifacts_do_not_follow_user_workspace_symlinks(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    external = tmp_path / "external"
    workspace.mkdir()
    external.mkdir()
    (workspace / ".github").symlink_to(external)

    with pytest.raises(CopilotLaunchError, match="through symlink"):
        prepare_workspace_artifacts(
            CopilotLaunchConfig(
                session_id=SESSION_ID,
                workspace_dir=str(workspace),
                env={"KERNEL_SYSTEM_PROMPT": "managed prompt"},
            ),
        )

    assert list(external.iterdir()) == []


def test_session_scoped_skill_links_preserve_user_files(tmp_path: Path) -> None:
    staging = tmp_path / "staging"
    workspace = tmp_path / "workspace"
    (staging / "alpha").mkdir(parents=True)
    (staging / "beta").mkdir()
    user_skill = workspace / ".github/skills/beta"
    user_skill.mkdir(parents=True)
    (user_skill / "SKILL.md").write_text("user skill", encoding="utf-8")

    artifacts = prepare_workspace_artifacts(
        CopilotLaunchConfig(
            session_id=SESSION_ID,
            workspace_dir=str(workspace),
            env={
                "KERNEL_SKILLS_STAGING_DIR": str(staging),
                "KERNEL_ENABLED_SKILLS": "alpha,beta",
            },
        ),
    )

    assert (workspace / ".github/skills/alpha").is_symlink()
    assert not user_skill.is_symlink()
    assert (user_skill / "SKILL.md").read_text(encoding="utf-8") == "user skill"
    assert artifacts.owned_relative_paths == (".github/skills/alpha",)


def test_different_sessions_keep_independent_enabled_skill_sets(
    tmp_path: Path,
) -> None:
    staging = tmp_path / "staging"
    (staging / "alpha").mkdir(parents=True)
    (staging / "beta").mkdir()
    workspace_a = tmp_path / "workspace-a"
    workspace_b = tmp_path / "workspace-b"

    for workspace, enabled, session_id in (
        (workspace_a, "alpha", SESSION_ID),
        (workspace_b, "beta", OTHER_SESSION_ID),
    ):
        prepare_workspace_artifacts(
            CopilotLaunchConfig(
                session_id=session_id,
                workspace_dir=str(workspace),
                env={
                    "KERNEL_SKILLS_STAGING_DIR": str(staging),
                    "KERNEL_ENABLED_SKILLS": enabled,
                },
            ),
        )

    assert (workspace_a / ".github/skills/alpha").is_symlink()
    assert not (workspace_a / ".github/skills/beta").exists()
    assert (workspace_b / ".github/skills/beta").is_symlink()
    assert not (workspace_b / ".github/skills/alpha").exists()


def test_skill_reconciliation_replaces_only_owned_links(tmp_path: Path) -> None:
    staging = tmp_path / "staging"
    workspace = tmp_path / "workspace"
    external = tmp_path / "external"
    (staging / "alpha").mkdir(parents=True)
    (staging / "beta").mkdir()
    external.mkdir()
    skills_dir = workspace / ".github/skills"
    skills_dir.mkdir(parents=True)
    (skills_dir / "alpha").symlink_to(staging / "alpha")
    (skills_dir / "user-link").symlink_to(external)

    prepare_workspace_artifacts(
        CopilotLaunchConfig(
            session_id=SESSION_ID,
            workspace_dir=str(workspace),
            env={
                "KERNEL_SKILLS_STAGING_DIR": str(staging),
                "KERNEL_ENABLED_SKILLS": "beta",
            },
        ),
    )

    assert not (skills_dir / "alpha").exists()
    assert (skills_dir / "beta").is_symlink()
    assert (skills_dir / "user-link").is_symlink()


def test_skill_reconciliation_removes_only_owned_legacy_shared_links(
    tmp_path: Path,
) -> None:
    staging = tmp_path / "staging"
    workspace = tmp_path / "workspace"
    legacy = tmp_path / "legacy"
    external = tmp_path / "external"
    (staging / "alpha").mkdir(parents=True)
    legacy.mkdir()
    external.mkdir()
    (legacy / "alpha").symlink_to(staging / "alpha")
    (legacy / "user-link").symlink_to(external)

    prepare_workspace_artifacts(
        CopilotLaunchConfig(
            session_id=SESSION_ID,
            workspace_dir=str(workspace),
            env={
                "KERNEL_SKILLS_STAGING_DIR": str(staging),
                "KERNEL_LEGACY_COPILOT_SKILLS_DIR": str(legacy),
                "KERNEL_ENABLED_SKILLS": "alpha",
            },
        ),
    )

    assert not (legacy / "alpha").exists()
    assert (legacy / "user-link").is_symlink()
    assert (workspace / ".github/skills/alpha").is_symlink()
