"""Consume the resolved effective Git Agent configuration from client_service.

The desired Git Agent configuration is authored declaratively in the
client_service ``ConfigDocument`` (branch/ref policy, remote/patch URLs,
validation command, and review agent). Its secret-backed leaves are resolved by
client_service and served over a private, token-authenticated internal endpoint
so resolved secret values are never exposed publicly. This module fetches that
effective configuration and folds it into :class:`Settings` so the applied
configuration actually drives Git Agent behavior.

Resolved values are never logged.
"""

from __future__ import annotations

import dataclasses
import shlex
from typing import TYPE_CHECKING, cast

import httpx

if TYPE_CHECKING:
    from git_agent.config import Settings

EFFECTIVE_CONFIG_PATH = "/internal/git-agent/effective-config"
INTERNAL_TOKEN_HEADER = "X-Internal-Token"  # noqa: S105 (header name, not a secret)
_HTTP_OK = 200
_HTTP_CONFLICT = 409


class EffectiveConfigError(RuntimeError):
    """Raised when the effective Git Agent config cannot be resolved."""


class EffectiveConfigUnresolvedError(EffectiveConfigError):
    """Raised when the effective config references secrets that are unset.

    This is a *fail-closed* signal (HTTP 409 from client_service): the caller
    must never continue operating with previously resolved (decrypted) values.
    It is deliberately distinct from a transient fetch failure so the caller can
    revert to immutable environment defaults rather than preserve stale secrets.
    """


def _as_str_tuple(value: object) -> tuple[str, ...]:
    if not isinstance(value, list):
        return ()
    items = cast("list[object]", value)
    strings: list[str] = [i for i in items if isinstance(i, str) and i]
    return tuple(strings)


def apply_effective_config(settings: Settings, payload: object) -> Settings:
    """Fold a resolved effective-config payload into ``settings``.

    Only fields present in the payload override the current settings. Unknown or
    absent fields leave the corresponding setting unchanged. A ``configured:
    false`` payload leaves settings untouched.
    """
    if not isinstance(payload, dict):
        msg = "effective config payload must be a JSON object"
        raise EffectiveConfigError(msg)
    data = cast("dict[str, object]", payload)
    if not data.get("configured", False):
        return settings

    updates: dict[str, object] = {}

    enabled = data.get("enabled")
    if isinstance(enabled, bool):
        updates["enabled"] = enabled

    _fold_str(updates, "review_agent_id", data.get("reviewAgent"))
    _fold_str(updates, "default_branch", data.get("defaultBranch"))
    _fold_str(updates, "remote_url", data.get("remoteUrl"))
    _fold_str(updates, "patch_url", data.get("patchUrl"))

    # The ref policy is authoritative when configured: an explicitly empty
    # prefix/ref list must replace the permissive env default (not be ignored).
    # We distinguish an absent key (leave unchanged) from a present-but-empty
    # list (apply the empty policy).
    if "allowedRefPrefixes" in data:
        updates["allowed_ref_prefixes"] = _as_str_tuple(data.get("allowedRefPrefixes"))
    if "allowedRefs" in data:
        updates["allowed_refs"] = _as_str_tuple(data.get("allowedRefs"))

    validation_command = data.get("validationCommand")
    if isinstance(validation_command, str) and validation_command.strip():
        updates["validation_command"] = tuple(shlex.split(validation_command))

    return dataclasses.replace(settings, **updates)


def _fold_str(updates: dict[str, object], field: str, value: object) -> None:
    """Fold a non-empty string ``value`` into ``updates`` under ``field``."""
    if isinstance(value, str) and value:
        updates[field] = value


async def fetch_effective_config(base_url: str, token: str) -> object:
    """Fetch the resolved effective config from the internal endpoint.

    Raises :class:`EffectiveConfigError` when the endpoint reports unset secrets
    (HTTP 409) or is otherwise unreachable/invalid. Error messages never include
    resolved secret values.
    """
    url = f"{base_url.rstrip('/')}{EFFECTIVE_CONFIG_PATH}"
    try:
        async with httpx.AsyncClient(timeout=30.0) as client:
            response = await client.get(url, headers={INTERNAL_TOKEN_HEADER: token})
    except httpx.HTTPError as exc:
        msg = "could not reach client_service to resolve Git Agent config"
        raise EffectiveConfigError(msg) from exc

    if response.status_code == _HTTP_CONFLICT:
        # The body lists which secret names/fields are unset (no values).
        try:
            body = response.json()
        except ValueError:
            body = {}
        missing: object = None
        if isinstance(body, dict):
            missing = cast("dict[str, object]", body).get("missing_secrets")
        msg = f"Git Agent config references unset secrets: {missing}"
        raise EffectiveConfigUnresolvedError(msg)
    if response.status_code != _HTTP_OK:
        msg = (
            "client_service effective-config endpoint returned HTTP "
            f"{response.status_code}"
        )
        raise EffectiveConfigError(msg)
    try:
        return response.json()
    except ValueError as exc:
        msg = "client_service effective-config response was not valid JSON"
        raise EffectiveConfigError(msg) from exc


async def load_effective_settings(settings: Settings) -> Settings:
    """Return settings enriched with the resolved effective config.

    When the internal token is configured the effective config is fetched and
    applied; otherwise ``settings`` is returned unchanged.
    """
    token = settings.client_service_internal_token
    if not token:
        return settings
    payload = await fetch_effective_config(settings.client_service_url, token)
    return apply_effective_config(settings, payload)
