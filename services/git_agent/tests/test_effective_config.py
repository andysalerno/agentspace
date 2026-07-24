from __future__ import annotations

import asyncio
import sys
from typing import TYPE_CHECKING

import pytest

from git_agent.app import AppState
from git_agent.config import Settings
from git_agent.effective_config import (
    EffectiveConfigError,
    EffectiveConfigUnresolvedError,
    apply_effective_config,
    fetch_effective_config,
)
from git_agent.patch_parser import (
    PatchValidationError,
    is_protected_ref,
    normalize_target_ref,
)

# ``git_agent.__init__`` rebinds the ``app`` attribute to a FastAPI instance, so
# reach the submodule through ``sys.modules`` to monkeypatch its functions.
app_module = sys.modules["git_agent.app"]

if TYPE_CHECKING:
    from pathlib import Path


def test_apply_effective_config_honors_enabled_flag() -> None:
    # gitAgent.enabled from the effective payload folds into settings so patch
    # operations can be gated.
    settings = Settings()
    assert settings.enabled is True
    disabled = apply_effective_config(settings, {"configured": True, "enabled": False})
    assert disabled.enabled is False
    reenabled = apply_effective_config(disabled, {"configured": True, "enabled": True})
    assert reenabled.enabled is True


def _branch_settings(tmp_path: Path, **overrides: object) -> Settings:
    base: dict[str, object] = {
        "repo_path": tmp_path / "repo.git",
        "db_path": tmp_path / "state.sqlite3",
        "scratch_path": tmp_path / "worktrees",
        "data_path": tmp_path,
    }
    base.update(overrides)
    return Settings(**base)  # type: ignore[arg-type]


def test_refresh_rebuilds_from_base_and_clears_stale(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Each refresh rebuilds from the immutable base settings, so switching to
    # configured:false reverts previously resolved policy/URLs to base defaults.
    base = _branch_settings(
        tmp_path,
        default_branch="main",
        allowed_ref_prefixes=("wip/",),
        remote_url=None,
    )
    state = AppState(base, reviewer=None)

    payloads: list[dict[str, object]] = [
        {
            "configured": True,
            "defaultBranch": "trunk",
            "allowedRefPrefixes": ["feature/"],
            "remoteUrl": "http://git/repo.git",
        },
        {"configured": False},
    ]

    async def fake_load(settings: Settings) -> Settings:
        # The refresh rebuilds from base settings each time (proven by the
        # configured:false revert below clearing all resolved values).
        return apply_effective_config(settings, payloads.pop(0))

    monkeypatch.setattr(app_module, "load_effective_settings", fake_load)

    asyncio.run(state.refresh_effective_config())
    assert state.settings.default_branch == "trunk"
    assert state.settings.allowed_ref_prefixes == ("feature/",)
    assert state.settings.remote_url == "http://git/repo.git"

    # configured:false rebuilds from base, clearing the resolved values.
    asyncio.run(state.refresh_effective_config())
    assert state.settings.default_branch == "main"
    assert state.settings.allowed_ref_prefixes == ("wip/",)
    assert state.settings.remote_url is None


def test_refresh_unresolved_secret_fails_closed_to_base(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # When a referenced secret becomes unset, the refresh must fail closed and
    # never preserve the previously resolved (decrypted) value.
    base = _branch_settings(tmp_path, default_branch="main", remote_url=None)
    state = AppState(base, reviewer=None)

    async def load_resolved(settings: Settings) -> Settings:
        return apply_effective_config(
            settings,
            {
                "configured": True,
                "defaultBranch": "trunk",
                "remoteUrl": "http://secret/repo.git",
            },
        )

    monkeypatch.setattr(app_module, "load_effective_settings", load_resolved)
    asyncio.run(state.refresh_effective_config())
    assert state.settings.remote_url == "http://secret/repo.git"
    assert state.settings.default_branch == "trunk"

    async def load_unresolved(_settings: Settings) -> Settings:
        msg = "Git Agent config references unset secrets: ['GIT_REMOTE']"
        raise EffectiveConfigUnresolvedError(msg)

    monkeypatch.setattr(app_module, "load_effective_settings", load_unresolved)
    asyncio.run(state.refresh_effective_config())
    assert state.settings.remote_url is None
    assert state.settings.default_branch == "main"


def test_refresh_transient_error_keeps_resolved_values(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A transient (non-secret) fetch failure must not crash or clear the
    # currently resolved settings.
    base = _branch_settings(tmp_path, default_branch="main")
    state = AppState(base, reviewer=None)

    async def load_resolved(settings: Settings) -> Settings:
        return apply_effective_config(
            settings,
            {"configured": True, "defaultBranch": "trunk"},
        )

    monkeypatch.setattr(app_module, "load_effective_settings", load_resolved)
    asyncio.run(state.refresh_effective_config())
    assert state.settings.default_branch == "trunk"

    async def load_transient(_settings: Settings) -> Settings:
        msg = "client_service unreachable"
        raise EffectiveConfigError(msg)

    monkeypatch.setattr(app_module, "load_effective_settings", load_transient)
    asyncio.run(state.refresh_effective_config())
    assert state.settings.default_branch == "trunk"


def test_refresh_syncs_bare_repo_head_to_default_branch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # After a config apply changes the default branch, the bare repo HEAD must
    # follow so clones check out the new trunk.
    base = _branch_settings(tmp_path, default_branch="main")
    state = AppState(base, reviewer=None)
    state.initialize()
    assert state.git.status()["head_ref"] == "refs/heads/main"

    async def load_trunk(settings: Settings) -> Settings:
        return apply_effective_config(
            settings,
            {"configured": True, "defaultBranch": "trunk"},
        )

    monkeypatch.setattr(app_module, "load_effective_settings", load_trunk)
    asyncio.run(state.refresh_effective_config())
    assert state.git.status()["head_ref"] == "refs/heads/trunk"


def test_apply_effective_config_folds_fields() -> None:
    settings = Settings()
    payload = {
        "configured": True,
        "reviewAgent": "reviewer-bot",
        "defaultBranch": "trunk",
        "allowedRefPrefixes": ["feature/", "wip/"],
        "allowedRefs": ["refs/heads/release"],
        "remoteUrl": "http://git/repo.git",
        "patchUrl": "http://git/PatchRequest",
        "validationCommand": "just validate --fast",
    }
    updated = apply_effective_config(settings, payload)
    assert updated.review_agent_id == "reviewer-bot"
    assert updated.default_branch == "trunk"
    assert updated.allowed_ref_prefixes == ("feature/", "wip/")
    assert updated.allowed_refs == ("refs/heads/release",)
    assert updated.remote_url == "http://git/repo.git"
    assert updated.patch_url == "http://git/PatchRequest"
    assert updated.validation_command == ("just", "validate", "--fast")


def test_apply_effective_config_ignores_unconfigured_payload() -> None:
    settings = Settings(review_agent_id="original")
    updated = apply_effective_config(settings, {"configured": False})
    assert updated == settings


def test_apply_effective_config_rejects_non_object() -> None:
    with pytest.raises(EffectiveConfigError):
        apply_effective_config(Settings(), ["not", "an", "object"])


def test_ref_policy_honors_custom_default_branch_and_prefix() -> None:
    # The applied default branch becomes the protected ref.
    assert normalize_target_ref("trunk", default_branch="trunk") == "refs/heads/trunk"
    assert is_protected_ref("refs/heads/trunk", default_branch="trunk")
    assert not is_protected_ref("refs/heads/main", default_branch="trunk")

    # A configured prefix drives which non-protected refs are accepted.
    assert (
        normalize_target_ref(
            "feature/login",
            default_branch="trunk",
            allowed_prefixes=("feature/",),
        )
        == "refs/heads/feature/login"
    )
    # The old default prefix is no longer accepted once the policy changes.
    with pytest.raises(PatchValidationError):
        normalize_target_ref(
            "wip/x",
            default_branch="trunk",
            allowed_prefixes=("feature/",),
        )


def test_ref_policy_accepts_full_default_branch_ref() -> None:
    default_branch = "refs/heads/trunk"
    assert (
        normalize_target_ref("trunk", default_branch=default_branch)
        == "refs/heads/trunk"
    )
    assert (
        normalize_target_ref("refs/heads/trunk", default_branch=default_branch)
        == "refs/heads/trunk"
    )
    assert is_protected_ref("trunk", default_branch=default_branch)
    assert is_protected_ref("refs/heads/trunk", default_branch=default_branch)
    assert not is_protected_ref("refs/heads/main", default_branch=default_branch)


def test_ref_policy_defaults_match_legacy_behavior() -> None:
    assert normalize_target_ref("main") == "refs/heads/main"
    assert normalize_target_ref("wip/x") == "refs/heads/wip/x"
    assert is_protected_ref("refs/heads/main")


def test_fetch_effective_config_unreachable_is_actionable() -> None:
    # An unreachable client_service yields an actionable error, never a leak.
    with pytest.raises(EffectiveConfigError) as excinfo:
        asyncio.run(fetch_effective_config("http://127.0.0.1:9", "token"))
    assert "client_service" in str(excinfo.value)


def test_ref_policy_allows_exact_allowed_ref() -> None:
    # An exact allowedRefs entry is accepted on its canonical full form, and the
    # caller may pass either the branch-relative name or the full ref.
    assert (
        normalize_target_ref(
            "release/1.0",
            default_branch="main",
            allowed_prefixes=(),
            allowed_refs=("refs/heads/release/1.0",),
        )
        == "refs/heads/release/1.0"
    )
    assert (
        normalize_target_ref(
            "refs/heads/release/1.0",
            default_branch="main",
            allowed_prefixes=(),
            allowed_refs=("refs/heads/release/1.0",),
        )
        == "refs/heads/release/1.0"
    )


def test_ref_policy_allows_prefix_but_denies_others() -> None:
    assert (
        normalize_target_ref(
            "feature/login",
            default_branch="main",
            allowed_prefixes=("feature/",),
            allowed_refs=(),
        )
        == "refs/heads/feature/login"
    )
    with pytest.raises(PatchValidationError):
        normalize_target_ref(
            "hotfix/x",
            default_branch="main",
            allowed_prefixes=("feature/",),
            allowed_refs=(),
        )


def test_ref_policy_full_ref_prefix_no_double_prepend() -> None:
    # A configured full-ref prefix must match a full ref without double-prepending
    # refs/heads/ (regression: refs/heads/refs/heads/feature/...).
    assert (
        normalize_target_ref(
            "refs/heads/feature/login",
            default_branch="main",
            allowed_prefixes=("refs/heads/feature/",),
            allowed_refs=(),
        )
        == "refs/heads/feature/login"
    )


def test_ref_policy_empty_policy_permits_only_default_branch() -> None:
    # An explicitly empty policy (no prefixes, no exact refs) rejects everything
    # except the default branch.
    assert (
        normalize_target_ref(
            "main",
            default_branch="main",
            allowed_prefixes=(),
            allowed_refs=(),
        )
        == "refs/heads/main"
    )
    with pytest.raises(PatchValidationError):
        normalize_target_ref(
            "wip/x",
            default_branch="main",
            allowed_prefixes=(),
            allowed_refs=(),
        )


def test_apply_effective_config_honors_empty_ref_policy() -> None:
    # An explicitly empty policy from the ConfigDocument must replace the
    # permissive env default, not be ignored (regression).
    settings = Settings(allowed_ref_prefixes=("wip/",), allowed_refs=("refs/heads/x",))
    payload: dict[str, object] = {
        "configured": True,
        "allowedRefPrefixes": [],
        "allowedRefs": [],
    }
    updated = apply_effective_config(settings, payload)
    assert updated.allowed_ref_prefixes == ()
    assert updated.allowed_refs == ()


def test_apply_effective_config_absent_ref_keys_leave_settings() -> None:
    # Absent keys leave the current policy unchanged (distinct from empty lists).
    settings = Settings(allowed_ref_prefixes=("wip/",), allowed_refs=("refs/heads/x",))
    updated = apply_effective_config(settings, {"configured": True})
    assert updated.allowed_ref_prefixes == ("wip/",)
    assert updated.allowed_refs == ("refs/heads/x",)


def test_refresh_effective_config_takes_effect_between_operations(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A config apply / secret rotation must affect the next operation without a
    # restart: refresh_effective_config re-reads the effective config each time.
    settings = Settings(
        repo_path=tmp_path / "repo.git",
        db_path=tmp_path / "state.sqlite3",
        scratch_path=tmp_path / "worktrees",
        data_path=tmp_path,
        default_branch="main",
        allowed_ref_prefixes=("wip/",),
        allowed_refs=(),
    )
    state = AppState(settings, reviewer=None)

    responses = [
        Settings(
            repo_path=tmp_path / "repo.git",
            db_path=tmp_path / "state.sqlite3",
            scratch_path=tmp_path / "worktrees",
            data_path=tmp_path,
            default_branch="trunk",
            allowed_ref_prefixes=("feature/",),
            allowed_refs=(),
        ),
        Settings(
            repo_path=tmp_path / "repo.git",
            db_path=tmp_path / "state.sqlite3",
            scratch_path=tmp_path / "worktrees",
            data_path=tmp_path,
            default_branch="trunk",
            allowed_ref_prefixes=("release/",),
            allowed_refs=("refs/heads/hotfix",),
        ),
    ]

    async def fake_load(_settings: Settings) -> Settings:
        return responses.pop(0)

    monkeypatch.setattr(app_module, "load_effective_settings", fake_load)

    # First operation resolves the initial applied policy.
    asyncio.run(state.refresh_effective_config())
    assert state.settings.default_branch == "trunk"
    assert state.settings.allowed_ref_prefixes == ("feature/",)
    assert (
        normalize_target_ref(
            "feature/x",
            default_branch=state.settings.default_branch,
            allowed_prefixes=state.settings.allowed_ref_prefixes,
            allowed_refs=state.settings.allowed_refs,
        )
        == "refs/heads/feature/x"
    )

    # A later config apply changes the policy; the next operation picks it up.
    asyncio.run(state.refresh_effective_config())
    assert state.settings.allowed_ref_prefixes == ("release/",)
    assert state.settings.allowed_refs == ("refs/heads/hotfix",)
    with pytest.raises(PatchValidationError):
        normalize_target_ref(
            "feature/x",
            default_branch=state.settings.default_branch,
            allowed_prefixes=state.settings.allowed_ref_prefixes,
            allowed_refs=state.settings.allowed_refs,
        )
    assert (
        normalize_target_ref(
            "hotfix",
            default_branch=state.settings.default_branch,
            allowed_prefixes=state.settings.allowed_ref_prefixes,
            allowed_refs=state.settings.allowed_refs,
        )
        == "refs/heads/hotfix"
    )


def test_refresh_effective_config_survives_resolution_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A transient resolution failure must not crash the operation; the prior
    # settings remain in place.
    settings = Settings(
        repo_path=tmp_path / "repo.git",
        db_path=tmp_path / "state.sqlite3",
        scratch_path=tmp_path / "worktrees",
        data_path=tmp_path,
        default_branch="main",
    )
    state = AppState(settings, reviewer=None)

    async def failing_load(_settings: Settings) -> Settings:
        msg = "client_service unreachable"
        raise EffectiveConfigError(msg)

    monkeypatch.setattr(app_module, "load_effective_settings", failing_load)
    asyncio.run(state.refresh_effective_config())
    assert state.settings.default_branch == "main"
