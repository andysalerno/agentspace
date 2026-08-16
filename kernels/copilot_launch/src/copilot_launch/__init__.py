from __future__ import annotations

import hashlib
import logging
import os
import re
import uuid
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Mapping

logger = logging.getLogger(__name__)

DEFAULT_WORKSPACE_DIR = "/workspace"
DEFAULT_SKILLS_STAGING_DIR = "/mnt/all-skills"
NO_AUTH_API_KEY = "not-required"
PROFILE_DIR = PurePosixPath(".github/agents")
SKILLS_DIR = PurePosixPath(".github/skills")
TELEMETRY_DIR = PurePosixPath("/var/lib/agentspace/telemetry")

_PROVIDER_ENV_NAMES = (
    "COPILOT_PROVIDER_TYPE",
    "COPILOT_PROVIDER_BASE_URL",
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_BEARER_TOKEN",
    "COPILOT_PROVIDER_WIRE_API",
)
_CONNECTION_ENV_NAMES = (
    "CONNECTION_URL",
    "CONNECTION_API_KEY",
    "CONNECTION_API_FLAVOR",
)
_SECRET_ENV_NAMES = (
    "COPILOT_PROVIDER_API_KEY",
    "COPILOT_PROVIDER_BEARER_TOKEN",
    "CONNECTION_API_KEY",
)
_SAFE_SESSION_ID = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?")
_RESERVED_SESSION_ARGS = ("--resume", "--session-id")
_TELEMETRY_ENV_PREFIXES = ("OTEL_", "COPILOT_OTEL_")


class CopilotLaunchError(ValueError):
    """Raised when Copilot launch configuration cannot be applied safely."""


@dataclass(frozen=True, slots=True)
class CopilotLaunchConfig:
    session_id: str
    env: Mapping[str, str]
    additional_paths: tuple[str, ...] = ()
    workspace_dir: str = DEFAULT_WORKSPACE_DIR


@dataclass(frozen=True, slots=True)
class CopilotWorkspaceArtifacts:
    agent_name: str | None
    owned_relative_paths: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class CopilotLaunch:
    argv: tuple[str, ...]
    environment: dict[str, str]
    cwd: str
    artifacts: CopilotWorkspaceArtifacts
    redacted_argv: tuple[str, ...]


def build_chat_launch(
    config: CopilotLaunchConfig,
    prompt: str,
    *,
    process_env: Mapping[str, str] | None = None,
) -> CopilotLaunch:
    artifacts = prepare_workspace_artifacts(config)
    argv = build_chat_argv(config, prompt, agent_name=artifacts.agent_name)
    redacted = list(argv)
    redacted[2] = f"<prompt redacted: {len(prompt)} chars>"
    return CopilotLaunch(
        argv=argv,
        environment=build_copilot_environment(config.env, process_env=process_env),
        cwd=config.workspace_dir,
        artifacts=artifacts,
        redacted_argv=tuple(redacted),
    )


def build_interactive_launch(
    config: CopilotLaunchConfig,
    *,
    process_env: Mapping[str, str] | None = None,
    telemetry_file_path: str | None = None,
) -> CopilotLaunch:
    artifacts = prepare_workspace_artifacts(config)
    argv = build_interactive_argv(config, agent_name=artifacts.agent_name)
    environment = build_copilot_environment(config.env, process_env=process_env)
    if telemetry_file_path is not None:
        environment.update(
            _managed_telemetry_environment(config, telemetry_file_path),
        )
    return CopilotLaunch(
        argv=argv,
        environment=environment,
        cwd=config.workspace_dir,
        artifacts=artifacts,
        redacted_argv=argv,
    )


def build_chat_argv(
    config: CopilotLaunchConfig,
    prompt: str,
    *,
    agent_name: str | None = None,
) -> tuple[str, ...]:
    argv = [
        "copilot",
        "-p",
        prompt,
        "--output-format",
        "json",
        "--allow-all",
        "--no-ask-user",
        "--no-auto-update",
        "--no-color",
        "-s",
    ]
    _append_shared_args(argv, config, agent_name=agent_name)
    return tuple(argv)


def build_interactive_argv(
    config: CopilotLaunchConfig,
    *,
    agent_name: str | None = None,
) -> tuple[str, ...]:
    argv = ["copilot", "--allow-all", "--no-auto-update", "--mouse=on"]
    _append_shared_args(argv, config, agent_name=agent_name)
    return tuple(argv)


def build_copilot_environment(
    config_env: Mapping[str, str],
    *,
    process_env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    environment = dict(os.environ if process_env is None else process_env)
    environment.update(config_env)
    for name in tuple(environment):
        if name.startswith(_TELEMETRY_ENV_PREFIXES):
            environment.pop(name)

    connection_url = config_env.get("CONNECTION_URL")
    connection_api_key = config_env.get("CONNECTION_API_KEY")
    connection_api_flavor = config_env.get("CONNECTION_API_FLAVOR")

    for name in _PROVIDER_ENV_NAMES:
        environment.pop(name, None)

    if connection_url:
        wire_api = _provider_wire_api(connection_api_flavor)
        environment.update(
            {
                "COPILOT_PROVIDER_TYPE": "openai",
                "COPILOT_PROVIDER_BASE_URL": connection_url,
                "COPILOT_PROVIDER_API_KEY": connection_api_key or NO_AUTH_API_KEY,
                "COPILOT_PROVIDER_WIRE_API": wire_api,
            },
        )
    elif connection_api_key or connection_api_flavor:
        msg = "CONNECTION_URL is required when configuring a Copilot provider"
        raise CopilotLaunchError(msg)

    for name in _CONNECTION_ENV_NAMES:
        environment.pop(name, None)

    return environment


def _managed_telemetry_environment(
    config: CopilotLaunchConfig,
    telemetry_file_path: str,
) -> dict[str, str]:
    path = PurePosixPath(telemetry_file_path)
    if (
        not path.is_absolute()
        or path.parent != TELEMETRY_DIR
        or path.suffix != ".jsonl"
    ):
        msg = f"telemetry file must be a JSONL file in {TELEMETRY_DIR}"
        raise CopilotLaunchError(msg)
    try:
        uuid.UUID(path.stem)
    except ValueError as error:
        msg = "telemetry filename must use a UUID launch identity"
        raise CopilotLaunchError(msg) from error

    runtime_session_id = config.env.get("AGENTSPACE_SESSION_ID")
    if not runtime_session_id:
        msg = "AGENTSPACE_SESSION_ID is required for managed telemetry"
        raise CopilotLaunchError(msg)

    return {
        "COPILOT_OTEL_ENABLED": "true",
        "COPILOT_OTEL_EXPORTER_TYPE": "file",
        "COPILOT_OTEL_FILE_EXPORTER_PATH": str(path),
        "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT": "false",
        "OTEL_RESOURCE_ATTRIBUTES": f"agentspace.session.id={runtime_session_id}",
    }


def prepare_workspace_artifacts(
    config: CopilotLaunchConfig,
) -> CopilotWorkspaceArtifacts:
    workspace = Path(config.workspace_dir)
    owned_paths: list[str] = []
    agent_name = _reconcile_agent_profile(workspace, config)
    if agent_name is not None:
        owned_paths.append(str(PROFILE_DIR / f"{agent_name}.agent.md"))
    owned_paths.extend(_reconcile_skill_links(workspace, config.env))
    return CopilotWorkspaceArtifacts(
        agent_name=agent_name,
        owned_relative_paths=tuple(owned_paths),
    )


def agent_profile_name(session_id: str) -> str:
    normalized = session_id.lower()
    if _SAFE_SESSION_ID.fullmatch(normalized):
        identity = normalized
    else:
        identity = hashlib.sha256(session_id.encode()).hexdigest()[:24]
    return f"agentspace-{identity}"


def _append_shared_args(
    argv: list[str],
    config: CopilotLaunchConfig,
    *,
    agent_name: str | None,
) -> None:
    if not config.session_id:
        msg = "Copilot requires a durable session ID"
        raise CopilotLaunchError(msg)
    try:
        uuid.UUID(config.session_id)
    except ValueError as error:
        msg = f"Copilot session ID must be a UUID: {config.session_id!r}"
        raise CopilotLaunchError(msg) from error

    argv.append(f"--session-id={config.session_id}")

    model = config.env.get("COPILOT_MODEL")
    if model:
        argv.extend(("--model", model))

    reasoning_effort = config.env.get("COPILOT_REASONING_EFFORT")
    if reasoning_effort:
        argv.extend(("--effort", reasoning_effort))

    config_dir = config.env.get("COPILOT_CONFIG_DIR")
    if config_dir:
        argv.extend(("--config-dir", config_dir))

    if agent_name is None and config.env.get("KERNEL_SYSTEM_PROMPT", ""):
        agent_name = agent_profile_name(_artifact_identity(config))
    if agent_name is not None:
        argv.append(f"--agent={agent_name}")

    for path in (*config.additional_paths, *_split_paths_env(config.env)):
        argv.extend(("--add-dir", path))

    argv.extend(_extra_arg_tokens(config.env))
    argv.append(f"--secret-env-vars={','.join(_SECRET_ENV_NAMES)}")


def _provider_wire_api(api_flavor: str | None) -> str:
    if api_flavor == "chat_completions":
        return "completions"
    if api_flavor == "responses":
        return "responses"
    msg = (
        "CONNECTION_API_FLAVOR must be 'chat_completions' or 'responses' "
        "when configuring a Copilot provider"
    )
    raise CopilotLaunchError(msg)


def _split_paths_env(env: Mapping[str, str]) -> tuple[str, ...]:
    raw = env.get("COPILOT_ADDITIONAL_PATHS", "")
    if not raw:
        return ()
    parts = tuple(segment for segment in re.split(r"[\n;]+", raw) if segment)
    if len(parts) != 1 or ":" not in raw:
        return parts

    colon_parts = tuple(segment for segment in raw.split(":") if segment)
    if all(segment.startswith("/") for segment in colon_parts):
        return colon_parts
    return parts


def _extra_arg_tokens(env: Mapping[str, str]) -> tuple[str, ...]:
    args = tuple(arg for arg in env.get("COPILOT_EXTRA_ARGS", "").splitlines() if arg)
    for arg in args:
        if arg == "-r" or any(
            arg == reserved or arg.startswith(f"{reserved}=")
            for reserved in _RESERVED_SESSION_ARGS
        ):
            msg = f"COPILOT_EXTRA_ARGS cannot override session identity: {arg!r}"
            raise CopilotLaunchError(msg)
    return args


def _reconcile_agent_profile(
    workspace: Path,
    config: CopilotLaunchConfig,
) -> str | None:
    agent_name = agent_profile_name(_artifact_identity(config))
    profile_path = workspace / PROFILE_DIR / f"{agent_name}.agent.md"
    ownership_marker = f"<!-- agentspace-owned-profile:{agent_name} -->"
    prompt = config.env.get("KERNEL_SYSTEM_PROMPT", "")

    if not prompt:
        if _workspace_symlink(workspace, PROFILE_DIR) is not None:
            return None
        if _profile_is_owned(profile_path, ownership_marker):
            profile_path.unlink()
            logger.info("removed stale AgentSpace Copilot profile %s", profile_path)
        return None

    _reject_workspace_symlinks(workspace, PROFILE_DIR)
    if (profile_path.exists() or profile_path.is_symlink()) and not _profile_is_owned(
        profile_path,
        ownership_marker,
    ):
        msg = f"refusing to replace user-authored Copilot profile {profile_path}"
        raise CopilotLaunchError(msg)

    profile_path.parent.mkdir(parents=True, exist_ok=True)
    profile_path.write_text(
        "\n".join(
            (
                "---",
                f'name: "{agent_name}"',
                'description: "AgentSpace session profile"',
                "---",
                ownership_marker,
                prompt,
                "",
            ),
        ),
        encoding="utf-8",
    )
    logger.info(
        "wrote AgentSpace Copilot profile %s (%d chars)",
        profile_path,
        len(prompt),
    )
    return agent_name


def _profile_is_owned(profile_path: Path, ownership_marker: str) -> bool:
    if profile_path.is_symlink() or not profile_path.is_file():
        return False
    try:
        return ownership_marker in profile_path.read_text(encoding="utf-8")
    except OSError:
        return False


def _artifact_identity(config: CopilotLaunchConfig) -> str:
    return config.env.get("AGENTSPACE_SESSION_ID") or config.session_id


def _reconcile_skill_links(
    workspace: Path,
    env: Mapping[str, str],
) -> tuple[str, ...]:
    staging_raw = env.get("KERNEL_SKILLS_STAGING_DIR", "")
    if not staging_raw:
        return ()

    staging = Path(staging_raw)
    _remove_legacy_skill_links(env, staging)
    _reject_workspace_symlinks(workspace, SKILLS_DIR)
    target = workspace / SKILLS_DIR
    enabled = _enabled_skills(env)

    if target.exists() and not target.is_dir():
        msg = f"cannot project Copilot skills because {target} is not a directory"
        raise CopilotLaunchError(msg)
    if target.is_dir():
        _remove_stale_skill_links(target, staging, enabled)

    if not staging.is_dir():
        return ()

    selected = tuple(
        entry
        for entry in sorted(staging.iterdir())
        if entry.is_dir() and (enabled is None or entry.name in enabled)
    )
    if not selected:
        return ()

    target.mkdir(parents=True, exist_ok=True)
    owned_paths: list[str] = []
    for entry in selected:
        _validate_skill_name(entry.name)
        link = target / entry.name
        if link.is_symlink() and _link_points_into(link, staging):
            link.unlink()
        elif link.exists() or link.is_symlink():
            logger.warning("preserving user-authored Copilot skill path %s", link)
            continue
        link.symlink_to(entry)
        owned_paths.append(str(SKILLS_DIR / entry.name))
        logger.info("linked AgentSpace Copilot skill %s -> %s", link, entry)
    return tuple(owned_paths)


def _remove_legacy_skill_links(env: Mapping[str, str], staging: Path) -> None:
    legacy_raw = env.get("KERNEL_LEGACY_COPILOT_SKILLS_DIR", "")
    if not legacy_raw:
        return
    legacy = Path(legacy_raw)
    if legacy.is_symlink() or not legacy.is_dir():
        return
    for existing in legacy.iterdir():
        if existing.is_symlink() and _link_points_into(existing, staging):
            try:
                existing.unlink()
            except FileNotFoundError:
                continue
            logger.info("removed legacy shared Copilot skill link %s", existing)


def _remove_stale_skill_links(
    target: Path,
    staging: Path,
    enabled: set[str] | None,
) -> None:
    for existing in target.iterdir():
        if (
            existing.is_symlink()
            and _link_points_into(existing, staging)
            and (enabled is not None and existing.name not in enabled)
        ):
            existing.unlink()
            logger.info("removed stale AgentSpace Copilot skill link %s", existing)


def _enabled_skills(env: Mapping[str, str]) -> set[str] | None:
    raw = env.get("KERNEL_ENABLED_SKILLS")
    if raw is None:
        return None
    enabled = {name for name in raw.split(",") if name}
    for name in enabled:
        _validate_skill_name(name)
    return enabled


def _validate_skill_name(name: str) -> None:
    if not name or name in {".", ".."} or "/" in name or "\\" in name:
        msg = f"invalid enabled skill name: {name!r}"
        raise CopilotLaunchError(msg)


def _link_points_into(link: Path, staging: Path) -> bool:
    try:
        raw_target = link.readlink()
    except OSError:
        return False
    absolute_target = (
        raw_target if raw_target.is_absolute() else link.parent / raw_target
    ).resolve(strict=False)
    staging_root = staging.resolve(strict=False)
    return absolute_target == staging_root or staging_root in absolute_target.parents


def _reject_workspace_symlinks(workspace: Path, relative_dir: PurePosixPath) -> None:
    symlink = _workspace_symlink(workspace, relative_dir)
    if symlink is not None:
        msg = f"refusing to manage Copilot artifacts through symlink {symlink}"
        raise CopilotLaunchError(msg)


def _workspace_symlink(
    workspace: Path,
    relative_dir: PurePosixPath,
) -> Path | None:
    current = workspace
    for part in relative_dir.parts:
        current /= part
        if current.is_symlink():
            return current
    return None
