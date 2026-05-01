from __future__ import annotations

import json
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal, Protocol, cast

import httpx
from pydantic import BaseModel, ConfigDict, Field, StrictBool, field_validator

from git_agent.patch_parser import (
    ChangedLine,
    CommentSide,
    PatchAnalysis,
    normalize_patch_path,
)

REVIEW_WORKSPACE_ID = "git-agent"

if TYPE_CHECKING:
    from git_agent.config import Settings


class ReviewerError(RuntimeError):
    pass


class ReviewerResponseError(ReviewerError):
    pass


@dataclass(frozen=True, kw_only=True)
class ReviewerRawResponse:
    payload: object
    session_id: str | None = None


@dataclass(frozen=True, kw_only=True)
class ReviewContext:
    request_id: str
    target_ref: str
    base_sha: str | None
    service_worktree_path: str
    agent_worktree_path: str
    commit_message: str
    author: object | None
    requester: object | None
    argument: object | None
    analysis: PatchAnalysis


@dataclass(frozen=True, kw_only=True)
class ReviewDecision:
    accepted: bool
    summary: str
    comments: list[dict[str, object]]
    session_id: str | None = None


class Reviewer(Protocol):
    async def review(self, context: ReviewContext) -> ReviewerRawResponse: ...


class ReviewerCommentModel(BaseModel):
    model_config = ConfigDict(extra="forbid")

    path: str | None = None
    side: CommentSide = "general"
    line: int | None = Field(default=None, ge=1)
    message: str = Field(min_length=1)

    @field_validator("side", mode="before")
    @classmethod
    def _normalize_side(cls, value: object) -> object:
        if not isinstance(value, str):
            return value
        normalized = value.lower()
        aliases: dict[str, Literal["left", "right", "binary", "general"]] = {
            "old": "left",
            "deletion": "left",
            "deleted": "left",
            "new": "right",
            "addition": "right",
            "added": "right",
            "file": "general",
        }
        return aliases.get(normalized, normalized)


def _new_comment_list() -> list[ReviewerCommentModel]:
    return []


class ReviewerDecisionModel(BaseModel):
    model_config = ConfigDict(extra="forbid")

    accepted: StrictBool
    summary: str = Field(min_length=1)
    comments: list[ReviewerCommentModel] = Field(default_factory=_new_comment_list)


class AutoReviewer:
    def __init__(self, mode: Literal["auto_accept", "auto_reject", "invalid"]) -> None:
        self._mode = mode

    async def review(self, context: ReviewContext) -> ReviewerRawResponse:
        if self._mode == "auto_accept":
            return ReviewerRawResponse(
                payload={
                    "accepted": True,
                    "summary": "Auto-accepted by GitAgent test reviewer.",
                    "comments": [],
                },
            )
        if self._mode == "invalid":
            return ReviewerRawResponse(
                payload={"summary": "This deliberately omits the accepted field."},
            )
        comment: dict[str, object] = {
            "side": "general",
            "message": "Auto-rejected by GitAgent test reviewer.",
        }
        if context.argument is not None:
            comment["message"] = (
                "Auto-rejected by GitAgent test reviewer after considering argument."
            )
        return ReviewerRawResponse(
            payload={
                "accepted": False,
                "summary": "Auto-rejected by GitAgent test reviewer.",
                "comments": [comment],
            },
        )


class ClientServiceReviewer:
    def __init__(self, *, base_url: str, agent_id: str) -> None:
        self._base_url = base_url.rstrip("/")
        self._agent_id = agent_id

    async def review(self, context: ReviewContext) -> ReviewerRawResponse:
        prompt = build_review_prompt(context)
        try:
            async with httpx.AsyncClient(timeout=120.0) as client:
                config_response = await client.get(f"{self._base_url}/git-agent/config")
                config_response.raise_for_status()
                review_agent_id = (
                    _extract_review_agent_id(config_response.json()) or self._agent_id
                )
                session_response = await client.post(
                    f"{self._base_url}/sessions",
                    json={
                        "agent_id": review_agent_id,
                        "workspace_mounts": [
                            {"workspace_id": REVIEW_WORKSPACE_ID, "mode": "rw"},
                        ],
                    },
                )
                session_response.raise_for_status()
                session_payload = session_response.json()
                session_id = _extract_session_id(session_payload)
                if session_id is None:
                    msg = "client_service did not return a review session id"
                    raise ReviewerError(msg)
                message_response = await client.post(
                    f"{self._base_url}/sessions/{session_id}/messages",
                    json={"message": prompt},
                )
                message_response.raise_for_status()
                return ReviewerRawResponse(
                    payload=_extract_reviewer_payload(message_response.json()),
                    session_id=session_id,
                )
        except (httpx.HTTPError, ValueError) as exc:
            msg = "client_service review request failed"
            raise ReviewerError(msg) from exc


def reviewer_from_settings(settings: Settings) -> Reviewer:
    if settings.review_mode in {"auto_accept", "auto_reject", "invalid"}:
        return AutoReviewer(
            cast(
                "Literal['auto_accept', 'auto_reject', 'invalid']",
                settings.review_mode,
            ),
        )
    if not settings.review_agent_id:
        msg = "GITAGENT_REVIEW_AGENT_ID is required when GITAGENT_REVIEW_MODE=client"
        raise ValueError(msg)
    return ClientServiceReviewer(
        base_url=settings.client_service_url,
        agent_id=settings.review_agent_id,
    )


def validate_reviewer_response(
    raw: ReviewerRawResponse,
    analysis: PatchAnalysis,
) -> ReviewDecision:
    payload = _coerce_payload(raw.payload)
    try:
        model = ReviewerDecisionModel.model_validate(payload)
    except ValueError as exc:
        msg = (
            "reviewer response must be strict JSON with accepted, summary, and comments"
        )
        raise ReviewerResponseError(msg) from exc
    comments = [_validate_comment(comment, analysis) for comment in model.comments]
    return ReviewDecision(
        accepted=model.accepted,
        summary=model.summary,
        comments=comments,
        session_id=raw.session_id,
    )


def _validate_comment(
    comment: ReviewerCommentModel,
    analysis: PatchAnalysis,
) -> dict[str, object]:
    path = normalize_patch_path(comment.path) if comment.path is not None else None
    if comment.side in {"left", "right"}:
        if path is None or comment.line is None:
            msg = "line comments must include path, side, and line"
            raise ReviewerResponseError(msg)
        changed_line = ChangedLine(
            path=path,
            side=cast("Literal['left', 'right']", comment.side),
            line=comment.line,
        )
        if changed_line not in analysis.changed_lines:
            msg = "reviewer line comment does not map to a changed line"
            raise ReviewerResponseError(msg)
        return {
            "path": path,
            "side": comment.side,
            "line": comment.line,
            "message": comment.message,
        }

    if comment.side == "binary":
        if (
            path is None
            or path not in analysis.binary_paths
            or comment.line is not None
        ):
            msg = "binary comments must reference a binary patch path without a line"
            raise ReviewerResponseError(msg)
        return {"path": path, "side": "binary", "message": comment.message}

    if comment.line is not None:
        msg = "general comments must not include a line"
        raise ReviewerResponseError(msg)
    if path is not None and path not in analysis.paths:
        msg = "general path comments must reference a changed path"
        raise ReviewerResponseError(msg)
    result: dict[str, object] = {"side": "general", "message": comment.message}
    if path is not None:
        result["path"] = path
    return result


def _coerce_payload(payload: object) -> object:
    if isinstance(payload, dict):
        mapping = cast("dict[str, object]", payload)
        if "accepted" in mapping:
            return mapping
    if isinstance(payload, str):
        return json.loads(_extract_json_object(payload))
    if isinstance(payload, dict):
        extracted = _extract_reviewer_payload(cast("dict[str, object]", payload))
        if extracted is payload:
            return cast("dict[str, object]", payload)
        return _coerce_payload(extracted)
    msg = "reviewer response is not a JSON object or JSON text"
    raise ReviewerResponseError(msg)


def _extract_json_object(text: str) -> str:
    stripped = text.strip()
    if stripped.startswith("```"):
        stripped = stripped.strip("`")
        if stripped.startswith("json"):
            stripped = stripped[4:].strip()
    start = stripped.find("{")
    end = stripped.rfind("}")
    if start == -1 or end == -1 or end <= start:
        msg = "reviewer response did not contain a JSON object"
        raise ReviewerResponseError(msg)
    return stripped[start : end + 1]


def _extract_session_id(payload: object) -> str | None:
    if not isinstance(payload, dict):
        return None
    mapping = cast("dict[str, object]", payload)
    for key in ("id", "session_id", "sessionId"):
        value = mapping.get(key)
        if isinstance(value, str) and value:
            return value
    return None


def _extract_review_agent_id(payload: object) -> str | None:
    if not isinstance(payload, dict):
        return None
    mapping = cast("dict[str, object]", payload)
    value = mapping.get("review_agent_id")
    if isinstance(value, str) and value:
        return value
    reviewer = mapping.get("reviewer")
    if isinstance(reviewer, dict):
        reviewer_mapping = cast("dict[str, object]", reviewer)
        value = reviewer_mapping.get("agent_id")
        if isinstance(value, str) and value:
            return value
    return None


def _extract_reviewer_payload(payload: object) -> object:
    if isinstance(payload, str):
        return payload
    if isinstance(payload, dict):
        mapping = cast("dict[str, object]", payload)
        if "accepted" in mapping:
            return mapping
        for key in (
            "review",
            "decision",
            "assistant_response",
            "response",
            "content",
            "message",
            "text",
        ):
            value = mapping.get(key)
            if value is not None:
                return value
        events = mapping.get("events")
        if isinstance(events, list):
            for event in reversed(cast("list[object]", events)):
                extracted = _extract_reviewer_payload(event)
                if extracted is not event:
                    return extracted
        return mapping
    return payload


def build_review_prompt(context: ReviewContext) -> str:
    argument_text = json.dumps(context.argument, sort_keys=True, default=str)
    return (
        "You are GitAgent's final reviewer. Decide whether to accept this patch. "
        "The full patch is intentionally not included in this prompt. Review it by "
        "exploring the dedicated git worktree mounted in your session, and inspect "
        "the diff incrementally with git commands instead of relying on a pasted "
        "diff. Do not modify files in the review worktree. "
        'Return only JSON with this schema: {"accepted": boolean, '
        '"summary": string, "comments": [{"path": string|null, '
        '"side": "left"|"right"|"binary"|"general", '
        '"line": integer|null, "message": string}]}. '
        "Line comments must point to changed diff lines exactly; verify line "
        "numbers with `git diff` before commenting. "
        "Binary files are allowed but discouraged; use side=binary without line "
        "when commenting on a binary patch. Each independent project in this "
        "monorepo must provide a justfile recipe named validate; require that "
        "for new subprojects. GitAgent is the final authority; no human override. "
        "If rejecting because the patch is stale or conflicted, tell the submitter "
        "to fetch latest, rebase, and resubmit.\n\n"
        f"PatchRequest: {context.request_id}\n"
        f"Target ref: {context.target_ref}\n"
        f"Base sha: {context.base_sha}\n"
        f"Commit message: {context.commit_message}\n"
        f"Author: {json.dumps(context.author, sort_keys=True, default=str)}\n"
        f"Requester: {json.dumps(context.requester, sort_keys=True, default=str)}\n"
        f"Argument/appeal: {argument_text}\n"
        f"Review worktree: {context.agent_worktree_path}\n\n"
        "Suggested workflow:\n"
        f"1. `cd {context.agent_worktree_path}`\n"
        "2. `git status --short`\n"
        "3. `git diff --stat HEAD`\n"
        "4. `git diff --name-only HEAD`\n"
        "5. Inspect suspicious files with `git diff HEAD -- <path>` or narrower "
        "`git diff -U40 HEAD -- <path>` commands.\n"
        "6. Read surrounding source and related files as needed before returning "
        "the JSON decision."
    )
