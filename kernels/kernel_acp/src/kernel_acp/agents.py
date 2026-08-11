"""Agent backends for the ACP kernel.

The ACP kernel speaks Agent Client Protocol to any compliant stdio server. Each
supported server still needs AgentSpace-specific provisioning: pointing the CLI
at the session's Connection (base URL, API key, model) and installing the
agent's system prompt. That provisioning is what an ``AcpAgent`` backend owns.

Select a backend with ``KERNEL_ACP_AGENT`` (``opencode`` by default). The
launched command can always be overridden with ``KERNEL_ACP_COMMAND``.
"""

from __future__ import annotations

import json
import logging
import os
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Protocol, cast

if TYPE_CHECKING:
    from collections.abc import Mapping

logger = logging.getLogger(__name__)

OPENCODE_AGENT = "opencode"
PI_AGENT = "pi"
DEFAULT_AGENT = OPENCODE_AGENT

CHAT_COMPLETIONS_FLAVOR = "chat_completions"
RESPONSES_FLAVOR = "responses"
DEFAULT_API_FLAVOR = CHAT_COMPLETIONS_FLAVOR

OPENCODE_CUSTOM_AGENT_NAME = "custom"
OPENCODE_PROVIDER_NAME = "customprovider"
PI_PROVIDER_NAME = "customprovider"
# Where agent_host mounts AgentSpace-managed skills for the ACP harness. See
# ``skills_mount_path`` in services/agent_host_rs/src/docker_runtime.rs.
PI_SKILLS_DIR = "/workspace/.agents/skills"

_OPENCODE_PROVIDER_NPM_BY_API_FLAVOR = {
    CHAT_COMPLETIONS_FLAVOR: "@ai-sdk/openai-compatible",
    RESPONSES_FLAVOR: "@ai-sdk/openai",
}
_PI_API_BY_API_FLAVOR = {
    CHAT_COMPLETIONS_FLAVOR: "openai-completions",
    RESPONSES_FLAVOR: "openai-responses",
}
_OPENCODE_PERMISSION_CONFIG = {
    "*": "allow",
    "bash": {
        "*": "allow",
    },
    "webfetch": "deny",
    "doom_loop": "deny",
    "external_directory": {
        "*": "deny",
        "/tmp/**": "allow",  # noqa: S108
    },
    "websearch": "deny",
    "question": "deny",
    "lsp": "deny",
}


@dataclass(frozen=True, slots=True)
class ConnectionSettings:
    """Model endpoint settings resolved from the session environment."""

    base_url: str
    api_key: str
    model_name: str
    api_flavor: str

    @classmethod
    def from_env(cls, env: Mapping[str, str]) -> ConnectionSettings:
        base_url = (
            env.get("CONNECTION_URL")
            or env.get("KERNEL_ACP_BASE_URL")
            or env.get("KERNEL_OPENCODE_BASE_URL")
        )
        api_key = (
            env.get("CONNECTION_API_KEY")
            or env.get("KERNEL_ACP_API_KEY")
            or env.get("KERNEL_OPENCODE_API_KEY")
        )
        model_name = env.get("KERNEL_ACP_MODEL_NAME") or env.get(
            "KERNEL_OPENCODE_MODEL_NAME",
        )
        required = {
            "CONNECTION_URL": base_url,
            "CONNECTION_API_KEY": api_key,
            "KERNEL_ACP_MODEL_NAME": model_name,
        }
        missing = [name for name, value in required.items() if not value]
        if missing:
            msg = (
                "ACP kernel is missing required environment "
                f"variable(s): {', '.join(missing)}. Assign a Connection with "
                "a URL and API key, and set KERNEL_ACP_MODEL_NAME on the agent "
                "or kernel configuration."
            )
            raise ValueError(msg)
        api_flavor = (
            env.get("CONNECTION_API_FLAVOR")
            or env.get("KERNEL_ACP_API_FLAVOR")
            or DEFAULT_API_FLAVOR
        )
        return cls(
            base_url=cast("str", base_url),
            api_key=cast("str", api_key),
            model_name=cast("str", model_name),
            api_flavor=api_flavor,
        )


class AcpAgent(Protocol):
    """Provisioning strategy for one ACP-speaking CLI."""

    @property
    def name(self) -> str:
        """Value accepted by ``KERNEL_ACP_AGENT`` for this backend."""
        ...

    def default_command(self) -> list[str]:
        """Command used when ``KERNEL_ACP_COMMAND`` is not set."""
        ...

    def provision(self, env: Mapping[str, str]) -> None:
        """Write the CLI's on-disk configuration before it is spawned."""
        ...

    def process_env(
        self,
        env: dict[str, str],
        cmd: list[str],
    ) -> dict[str, str]:
        """Adjust the subprocess environment for the launched command."""
        ...


class OpencodeAgent:
    """opencode's built-in ACP server (``opencode acp``)."""

    @property
    def name(self) -> str:
        return OPENCODE_AGENT

    def default_command(self) -> list[str]:
        return ["opencode", "acp"]

    def provision(self, env: Mapping[str, str]) -> None:
        self.write_config(env)
        self.write_custom_agent_prompt(env)

    def process_env(self, env: dict[str, str], cmd: list[str]) -> dict[str, str]:
        if self._should_use_custom_default_agent(cmd):
            env["OPENCODE_CONFIG_CONTENT"] = self._config_content(
                env.get("OPENCODE_CONFIG_CONTENT"),
            )
        return env

    @property
    def config_path(self) -> Path:
        return Path.home() / ".config" / "opencode" / "opencode.json"

    @property
    def custom_agent_path(self) -> Path:
        return (
            Path.home()
            / ".config"
            / "opencode"
            / "agents"
            / f"{OPENCODE_CUSTOM_AGENT_NAME}.md"
        )

    def write_config(self, env: Mapping[str, str]) -> None:
        """Write opencode provider and permission config."""
        connection = ConnectionSettings.from_env(env)
        config_path = self.config_path
        config: dict[str, object] = {
            "$schema": "https://opencode.ai/config.json",
        }
        if config_path.exists():
            loaded = json.loads(config_path.read_text())
            if not isinstance(loaded, dict):
                msg = f"opencode config must be a JSON object: {config_path}"
                raise ValueError(msg)
            config = cast("dict[str, object]", loaded)
            config.setdefault("$schema", "https://opencode.ai/config.json")
        model_name = connection.model_name
        config["model"] = f"{OPENCODE_PROVIDER_NAME}/{model_name}"
        config["provider"] = {
            OPENCODE_PROVIDER_NAME: {
                "npm": self._provider_npm(connection.api_flavor),
                "name": OPENCODE_PROVIDER_NAME,
                "options": {
                    "baseURL": connection.base_url,
                    "apiKey": connection.api_key,
                },
                "models": {
                    model_name: {
                        "name": model_name,
                    },
                },
            },
        }
        config["permission"] = _OPENCODE_PERMISSION_CONFIG

        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps(config, indent=2))
        logger.info("wrote opencode config to %s", config_path)

    def write_custom_agent_prompt(self, env: Mapping[str, str]) -> None:
        """Write the AgentSpace system prompt as an opencode primary agent."""
        prompt = env.get("KERNEL_SYSTEM_PROMPT", "")
        path = self.custom_agent_path
        path.parent.mkdir(parents=True, exist_ok=True)
        content = ""
        if prompt.strip():
            content = (
                "---\n"
                "description: AgentSpace custom system prompt\n"
                "mode: primary\n"
                "---\n"
                f"{prompt}"
            )
        path.write_text(content)
        logger.info(
            "wrote opencode custom agent prompt to %s (%d chars)",
            path,
            len(prompt),
        )

    def _provider_npm(self, api_flavor: str) -> str:
        provider_npm = _OPENCODE_PROVIDER_NPM_BY_API_FLAVOR.get(api_flavor)
        if provider_npm is None:
            raise _invalid_api_flavor_error(_OPENCODE_PROVIDER_NPM_BY_API_FLAVOR)
        return provider_npm

    def _config_content(self, raw: str | None) -> str:
        config: dict[str, object] = {}
        if raw:
            parsed = json.loads(raw)
            if not isinstance(parsed, dict):
                msg = "OPENCODE_CONFIG_CONTENT must be a JSON object"
                raise ValueError(msg)
            config = cast("dict[str, object]", parsed)
        config["default_agent"] = OPENCODE_CUSTOM_AGENT_NAME
        return json.dumps(config, separators=(",", ":"))

    def _should_use_custom_default_agent(self, cmd: list[str]) -> bool:
        if not self._has_custom_agent_prompt():
            return False
        executable = Path(cmd[0]).name if cmd else ""
        return executable == "opencode" and "acp" in cmd[1:]

    def _has_custom_agent_prompt(self) -> bool:
        try:
            return bool(self.custom_agent_path.read_text().strip())
        except OSError:
            return False


class PiAgent:
    """The pi coding agent, fronted by the ``pi-acp`` adapter.

    pi has no native ACP server, so AgentSpace launches ``pi-acp``
    (https://github.com/svkozak/pi-acp), the ACP-registry adapter that spawns
    ``pi --mode rpc`` and translates its RPC stream into ACP. Provisioning
    writes pi's own config files: a custom provider in ``models.json``, session
    defaults in ``settings.json``, and the system prompt in ``SYSTEM.md``.
    """

    @property
    def name(self) -> str:
        return PI_AGENT

    def default_command(self) -> list[str]:
        return ["pi-acp"]

    def provision(self, env: Mapping[str, str]) -> None:
        self.write_models_config(env)
        self.write_settings(env)
        self.write_system_prompt(env)

    def process_env(self, env: dict[str, str], cmd: list[str]) -> dict[str, str]:
        del cmd
        # Startup network calls (version checks, catalog refreshes) only add
        # latency: models come from the models.json written above.
        for key, value in (
            ("PI_OFFLINE", "1"),
            ("PI_SKIP_VERSION_CHECK", "1"),
            ("PI_TELEMETRY", "0"),
        ):
            env.setdefault(key, value)
        return env

    def config_dir(self, env: Mapping[str, str]) -> Path:
        # pi reads this from its own process environment, which is the kernel's
        # environment merged with the session env, so honour both.
        override = env.get("PI_CODING_AGENT_DIR") or os.environ.get(
            "PI_CODING_AGENT_DIR",
        )
        if override:
            return Path(override)
        return Path.home() / ".pi" / "agent"

    def write_models_config(self, env: Mapping[str, str]) -> None:
        """Register the Connection as a custom pi provider."""
        connection = ConnectionSettings.from_env(env)
        path = self.config_dir(env) / "models.json"
        config = _load_json_object(path, "pi models config")
        providers = _as_dict(config.get("providers"))
        providers[PI_PROVIDER_NAME] = {
            "baseUrl": connection.base_url,
            "api": self._api_type(connection.api_flavor),
            "apiKey": _pi_literal(connection.api_key),
            "models": [
                {
                    "id": connection.model_name,
                    "name": connection.model_name,
                },
            ],
        }
        config["providers"] = providers
        _write_json(path, config)
        logger.info("wrote pi models config to %s", path)

    def write_settings(self, env: Mapping[str, str]) -> None:
        """Pin pi to the provisioned model and keep startup non-interactive."""
        connection = ConnectionSettings.from_env(env)
        path = self.config_dir(env) / "settings.json"
        settings = _load_json_object(path, "pi settings")
        settings["defaultProvider"] = PI_PROVIDER_NAME
        settings["defaultModel"] = connection.model_name
        # Leave project trust off. Trusting the workspace would also load
        # project-local `.pi/settings.json`, extensions and packages, which pi
        # executes at startup with this process's environment (including the
        # Connection API key) before the model takes a turn. AgentSpace-managed
        # skills are instead pointed at explicitly below, which loads them as
        # data without trusting anything else the workspace ships.
        settings["defaultProjectTrust"] = "never"
        settings["skills"] = self._skills(env, settings.get("skills"))
        settings["enableInstallTelemetry"] = False
        settings["quietStartup"] = True
        _write_json(path, settings)
        logger.info("wrote pi settings to %s", path)

    def _skills(self, env: Mapping[str, str], configured: object) -> list[str]:
        """Add the managed skills mount to any skill paths already configured."""
        skills: list[str] = []
        if isinstance(configured, list):
            skills = [
                entry
                for entry in cast("list[object]", configured)
                if isinstance(entry, str)
            ]
        skills_dir = env.get("KERNEL_ACP_SKILLS_DIR", PI_SKILLS_DIR)
        # pi warns about skill paths that resolve to nothing, and the mount is
        # absent whenever the session has no skills attached.
        if skills_dir and skills_dir not in skills and Path(skills_dir).is_dir():
            skills.append(skills_dir)
        return skills

    def write_system_prompt(self, env: Mapping[str, str]) -> None:
        """Replace pi's default system prompt with the AgentSpace prompt."""
        prompt = env.get("KERNEL_SYSTEM_PROMPT", "")
        path = self.config_dir(env) / "SYSTEM.md"
        if not prompt.strip():
            # An empty SYSTEM.md would blank pi's prompt instead of restoring
            # the default, so a stale file has to be removed outright.
            path.unlink(missing_ok=True)
            logger.info("cleared pi system prompt at %s", path)
            return
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(prompt)
        logger.info("wrote pi system prompt to %s (%d chars)", path, len(prompt))

    def _api_type(self, api_flavor: str) -> str:
        api_type = _PI_API_BY_API_FLAVOR.get(api_flavor)
        if api_type is None:
            raise _invalid_api_flavor_error(_PI_API_BY_API_FLAVOR)
        return api_type


AGENTS: dict[str, AcpAgent] = {
    OPENCODE_AGENT: OpencodeAgent(),
    PI_AGENT: PiAgent(),
}


def get_agent(name: str) -> AcpAgent:
    """Resolve an ACP agent backend by its ``KERNEL_ACP_AGENT`` name."""
    agent = AGENTS.get(name)
    if agent is None:
        valid = ", ".join(sorted(AGENTS))
        msg = f"KERNEL_ACP_AGENT must be one of: {valid}"
        raise ValueError(msg)
    return agent


def _invalid_api_flavor_error(flavors: Mapping[str, str]) -> ValueError:
    valid = ", ".join(flavors)
    return ValueError(f"CONNECTION_API_FLAVOR must be one of: {valid}")


def _pi_literal(value: str) -> str:
    """Escape a secret so pi's ``models.json`` resolver treats it literally.

    pi resolves ``apiKey`` as a config expression: ``$NAME``/``${NAME}`` is
    interpolated from the environment anywhere in the value, and a leading
    ``!`` runs the rest as a shell command. Writing a key verbatim would
    therefore mangle any key containing ``$`` and would execute a key starting
    with ``!``. ``$$`` and ``$!`` are pi's escapes for those two characters.
    """
    escaped = value.replace("$", "$$")
    if escaped.startswith("!"):
        escaped = f"${escaped}"
    return escaped


def _load_json_object(path: Path, description: str) -> dict[str, object]:
    if not path.exists():
        return {}
    loaded = json.loads(path.read_text())
    if not isinstance(loaded, dict):
        msg = f"{description} must be a JSON object: {path}"
        # ValueError, not TypeError: the kernel reports ValueError from
        # provisioning as a session error event.
        raise ValueError(msg)  # noqa: TRY004
    return cast("dict[str, object]", loaded)


def _write_json(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2))


def _as_dict(value: object) -> dict[str, object]:
    if isinstance(value, dict):
        return cast("dict[str, object]", value)
    return {}
