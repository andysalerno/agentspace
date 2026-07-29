# pyright: reportUnusedFunction=false
from __future__ import annotations

import hashlib
import subprocess
import uuid
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Annotated

from fastapi import FastAPI, HTTPException, Query, Request
from fastapi.responses import PlainTextResponse, Response
from pydantic import AliasChoices, BaseModel, ConfigDict, Field

from git_agent.config import Settings
from git_agent.git_backend import (
    GitBackend,
    PatchApplyError,
    PreparedPatch,
    RefUpdateError,
)
from git_agent.patch_parser import (
    NULL_SHA,
    PatchAnalysis,
    PatchValidationError,
    analyze_patch,
    is_empty_base_sha,
    is_protected_ref,
    normalize_target_ref,
    validate_patch_paths,
    validate_sha,
)
from git_agent.reviewer import (
    ReviewContext,
    ReviewDecision,
    Reviewer,
    ReviewerError,
    ReviewerResponseError,
    reviewer_from_settings,
    validate_reviewer_response,
)
from git_agent.storage import PatchRequestCreate, PatchRequestUpdate, PatchStore

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator
    from pathlib import Path


class PatchRequestBody(BaseModel):
    model_config = ConfigDict(populate_by_name=True)

    target_ref: str = "main"
    base_sha: str | None = None
    raw_patch: str = Field(
        min_length=1,
        validation_alias=AliasChoices("raw_patch", "patch"),
    )
    commit_message: str = Field(min_length=1)
    author: object | None = None
    requester: object | None = None
    argument: object | None = None
    response_to_request_id: str | None = None


class AppState:
    def __init__(self, settings: Settings, reviewer: Reviewer | None) -> None:
        self.settings = settings
        self.git = GitBackend(settings.repo_path, settings.scratch_path)
        self.store = PatchStore(settings.db_path)
        self.reviewer = reviewer or reviewer_from_settings(settings)

    def initialize(self) -> None:
        self.git.initialize()
        self.store.initialize()


def create_app(  # noqa: C901
    settings: Settings | None = None,
    reviewer: Reviewer | None = None,
) -> FastAPI:
    state = AppState(settings or Settings.from_env(), reviewer)

    @asynccontextmanager
    async def lifespan(_app: FastAPI) -> AsyncGenerator[None]:
        state.initialize()
        yield

    application = FastAPI(title="GitAgent", version="0.1.0", lifespan=lifespan)

    @application.get("/healthz")
    async def healthz() -> dict[str, str]:
        return {"status": "ok"}

    @application.get("/status")
    async def status() -> dict[str, object]:
        return state.git.status()

    @application.post("/PatchRequest")
    async def patch_request_camel(payload: PatchRequestBody) -> dict[str, object]:
        return await _handle_patch_request(payload, state)

    @application.post("/patch-requests")
    async def patch_request(payload: PatchRequestBody) -> dict[str, object]:
        return await _handle_patch_request(payload, state)

    @application.get("/patch-requests")
    async def list_patch_requests(
        limit: Annotated[int, Query(ge=1, le=500)] = 100,
    ) -> dict[str, object]:
        rows = state.store.list(limit=limit)
        return {
            "patch_requests": [row.to_dict(include_raw_patch=False) for row in rows],
        }

    @application.get("/patch-requests/{request_id}/raw")
    async def raw_patch(request_id: str) -> PlainTextResponse:
        row = state.store.get(request_id)
        if row is None:
            raise HTTPException(status_code=404, detail="patch request not found")
        return PlainTextResponse(row.raw_patch)

    @application.post("/patch-requests/{request_id}/rerun-review")
    async def rerun_review(request_id: str) -> dict[str, object]:
        return await _rerun_review(request_id, state)

    @application.get("/patch-requests/{request_id}")
    async def get_patch_request(request_id: str) -> dict[str, object]:
        row = state.store.get(request_id)
        if row is None:
            raise HTTPException(status_code=404, detail="patch request not found")
        return row.to_dict(include_raw_patch=True)

    @application.api_route("/{git_path:path}", methods=["GET", "POST"])
    async def git_http(git_path: str, request: Request) -> Response:
        body = await request.body()
        path_info = f"/{git_path}"
        try:
            backend_response = state.git.run_http_backend(
                path_info=path_info,
                query_string=str(request.url.query),
                method=request.method,
                body=body,
                content_type=request.headers.get("content-type"),
            )
        except FileNotFoundError as exc:
            raise HTTPException(status_code=404, detail=str(exc)) from exc
        except ValueError as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc
        return Response(
            content=backend_response.body,
            status_code=backend_response.status_code,
            headers=backend_response.headers,
        )

    return application


async def _handle_patch_request(
    payload: PatchRequestBody,
    state: AppState,
) -> dict[str, object]:
    request_id = uuid.uuid4().hex
    raw_patch = payload.raw_patch
    patch_hash = hashlib.sha256(raw_patch.encode()).hexdigest()
    base_sha = payload.base_sha.strip().lower() if payload.base_sha else None
    target_ref = payload.target_ref.strip()
    normalized_ref: str | None = None
    target_error: PatchValidationError | None = None
    try:
        normalized_ref = normalize_target_ref(target_ref)
    except PatchValidationError as exc:
        target_error = exc

    row = state.store.create(
        PatchRequestCreate(
            request_id=request_id,
            target_ref=normalized_ref or target_ref,
            base_sha=base_sha,
            raw_patch=raw_patch,
            patch_hash=patch_hash,
            commit_message=payload.commit_message,
            author=payload.author,
            requester=payload.requester,
            argument=payload.argument,
            response_to_request_id=payload.response_to_request_id,
            reviewer_id=state.settings.review_agent_id,
        ),
    )

    if target_error is not None:
        updated = state.store.update(
            row.request_id,
            PatchRequestUpdate(
                status="rejected",
                accepted=False,
                comments=[_general_comment(str(target_error), code="invalid_request")],
            ),
        )
        return updated.to_dict(include_raw_patch=True)

    try:
        normalized_base_sha = validate_sha(base_sha) if base_sha is not None else None
        analysis = analyze_patch(raw_patch)
        validate_patch_paths(analysis)
    except PatchValidationError as exc:
        updated = state.store.update(
            row.request_id,
            PatchRequestUpdate(
                status="rejected",
                accepted=False,
                comments=[_general_comment(str(exc), code="invalid_request")],
            ),
        )
        return updated.to_dict(include_raw_patch=True)

    if normalized_ref is None:
        msg = "target ref normalization unexpectedly failed"
        raise RuntimeError(msg)

    if is_protected_ref(normalized_ref):
        result = await _process_protected_patch(
            payload=payload,
            state=state,
            request_id=row.request_id,
            target_ref=normalized_ref,
            base_sha=normalized_base_sha,
            analysis=analysis,
        )
    else:
        result = _process_wip_patch(
            payload=payload,
            state=state,
            request_id=row.request_id,
            target_ref=normalized_ref,
            base_sha=normalized_base_sha,
        )
    result["line_indexes"] = analysis.line_indexes_json()
    result["binary_paths"] = sorted(analysis.binary_paths)
    return result


async def _rerun_review(request_id: str, state: AppState) -> dict[str, object]:
    row = state.store.get(request_id)
    if row is None:
        raise HTTPException(status_code=404, detail="patch request not found")
    if row.accepted or row.commit_sha is not None:
        raise HTTPException(
            status_code=409,
            detail="accepted patch requests cannot be re-reviewed",
        )

    try:
        target_ref = normalize_target_ref(row.target_ref)
        base_sha = validate_sha(row.base_sha) if row.base_sha is not None else None
        analysis = analyze_patch(row.raw_patch)
        validate_patch_paths(analysis)
    except PatchValidationError as exc:
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="rejected",
                accepted=False,
                comments=[_general_comment(str(exc), code="invalid_request")],
            ),
        )
        return updated.to_dict(include_raw_patch=True)

    if not is_protected_ref(target_ref):
        raise HTTPException(
            status_code=409,
            detail="wip patch requests skip review and cannot be re-reviewed",
        )

    payload = PatchRequestBody(
        target_ref=target_ref,
        base_sha=base_sha,
        raw_patch=row.raw_patch,
        commit_message=row.commit_message,
        author=row.author,
        requester=row.requester,
        argument=row.argument,
        response_to_request_id=row.response_to_request_id,
    )
    result = await _process_protected_patch(
        payload=payload,
        state=state,
        request_id=row.request_id,
        target_ref=target_ref,
        base_sha=base_sha,
        analysis=analysis,
    )
    result["line_indexes"] = analysis.line_indexes_json()
    result["binary_paths"] = sorted(analysis.binary_paths)
    return result


async def _process_protected_patch(  # noqa: PLR0911, PLR0913
    *,
    payload: PatchRequestBody,
    state: AppState,
    request_id: str,
    target_ref: str,
    base_sha: str | None,
    analysis: PatchAnalysis,
) -> dict[str, object]:
    if base_sha is None:
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="rejected",
                accepted=False,
                comments=[
                    _general_comment(
                        "Protected main requires base_sha. Fetch latest main, "
                        "rebase your work, and resubmit with that commit id.",
                        code="missing_base_sha",
                    ),
                ],
            ),
        )
        return updated.to_dict(include_raw_patch=True)

    head_before = state.git.get_ref(target_ref)
    stale_comment = _stale_comment(target_ref, base_sha, head_before)
    if head_before is None and not is_empty_base_sha(base_sha):
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="stale_base",
                accepted=False,
                comments=[stale_comment],
                head_before=head_before,
            ),
        )
        return updated.to_dict(include_raw_patch=True)
    if head_before is not None and base_sha != head_before:
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="stale_base",
                accepted=False,
                comments=[stale_comment],
                head_before=head_before,
            ),
        )
        return updated.to_dict(include_raw_patch=True)

    prepared = _prepare_or_conflict(
        state=state,
        request_id=request_id,
        base_sha=None if is_empty_base_sha(base_sha) else base_sha,
        raw_patch=payload.raw_patch,
        create_review_worktree=True,
        head_before=head_before,
    )
    if not isinstance(prepared, PreparedPatch):
        return prepared

    try:
        review_result = await _review_patch(
            payload=payload,
            state=state,
            request_id=request_id,
            target_ref=target_ref,
            base_sha=base_sha,
            analysis=analysis,
            prepared=prepared,
            head_before=head_before,
        )
        if isinstance(review_result, dict):
            return review_result
        if not review_result.accepted:
            updated = state.store.update(
                request_id,
                PatchRequestUpdate(
                    status="denied",
                    accepted=False,
                    comments=review_result.comments,
                    head_before=head_before,
                    reviewer_session_id=review_result.session_id,
                    reviewer_summary=review_result.summary,
                ),
            )
            return updated.to_dict(include_raw_patch=True)

        validation_failure = _run_validation_if_configured(
            state=state,
            request_id=request_id,
            prepared=prepared,
            head_before=head_before,
            reviewer_session_id=review_result.session_id,
            reviewer_summary=review_result.summary,
        )
        if validation_failure is not None:
            return validation_failure

        try:
            commit_sha = state.git.commit_prepared_patch(
                prepared=prepared,
                target_ref=target_ref,
                expected_old=head_before,
                message=payload.commit_message,
                author=payload.author,
            )
        except RefUpdateError:
            updated = state.store.update(
                request_id,
                PatchRequestUpdate(
                    status="stale_base",
                    accepted=False,
                    comments=[
                        _stale_comment(
                            target_ref,
                            base_sha,
                            state.git.get_ref(target_ref),
                        ),
                    ],
                    head_before=head_before,
                    reviewer_session_id=review_result.session_id,
                    reviewer_summary=review_result.summary,
                ),
            )
            return updated.to_dict(include_raw_patch=True)

        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="accepted",
                accepted=True,
                comments=review_result.comments,
                head_before=head_before,
                commit_sha=commit_sha,
                reviewer_session_id=review_result.session_id,
                reviewer_summary=review_result.summary,
            ),
        )
        return updated.to_dict(include_raw_patch=True)
    finally:
        prepared.cleanup()


def _process_wip_patch(
    *,
    payload: PatchRequestBody,
    state: AppState,
    request_id: str,
    target_ref: str,
    base_sha: str | None,
) -> dict[str, object]:
    head_before = state.git.get_ref(target_ref)
    base_for_apply: str | None
    if head_before is not None:
        if base_sha is not None and base_sha != head_before:
            updated = state.store.update(
                request_id,
                PatchRequestUpdate(
                    status="stale_base",
                    accepted=False,
                    comments=[_stale_comment(target_ref, base_sha, head_before)],
                    head_before=head_before,
                ),
            )
            return updated.to_dict(include_raw_patch=True)
        base_for_apply = head_before
    elif base_sha is not None and not is_empty_base_sha(base_sha):
        if not state.git.commit_exists(base_sha):
            updated = state.store.update(
                request_id,
                PatchRequestUpdate(
                    status="stale_base",
                    accepted=False,
                    comments=[
                        _general_comment(
                            "Target branch does not exist and base_sha is not known. "
                            "Fetch latest refs, rebase on an existing commit, and "
                            "resubmit.",
                            code="unknown_base",
                        ),
                    ],
                    head_before=head_before,
                ),
            )
            return updated.to_dict(include_raw_patch=True)
        base_for_apply = base_sha
    elif state.git.list_refs():
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="stale_base",
                accepted=False,
                comments=[
                    _general_comment(
                        "Target wip branch does not exist. Provide a base_sha from the "
                        "latest refs, rebase your work on it, and resubmit.",
                        code="missing_wip_base",
                    ),
                ],
                head_before=head_before,
            ),
        )
        return updated.to_dict(include_raw_patch=True)
    else:
        base_for_apply = None

    prepared = _prepare_or_conflict(
        state=state,
        request_id=request_id,
        base_sha=base_for_apply,
        raw_patch=payload.raw_patch,
        head_before=head_before,
    )
    if not isinstance(prepared, PreparedPatch):
        return prepared
    try:
        commit_sha = state.git.commit_prepared_patch(
            prepared=prepared,
            target_ref=target_ref,
            expected_old=head_before,
            message=payload.commit_message,
            author=payload.author,
        )
    except RefUpdateError:
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="stale_base",
                accepted=False,
                comments=[
                    _stale_comment(target_ref, base_sha, state.git.get_ref(target_ref)),
                ],
                head_before=head_before,
            ),
        )
        return updated.to_dict(include_raw_patch=True)
    finally:
        prepared.cleanup()

    updated = state.store.update(
        request_id,
        PatchRequestUpdate(
            status="accepted",
            accepted=True,
            comments=[],
            head_before=head_before,
            commit_sha=commit_sha,
        ),
    )
    return updated.to_dict(include_raw_patch=True)


def _prepare_or_conflict(  # noqa: PLR0913
    *,
    state: AppState,
    request_id: str,
    base_sha: str | None,
    raw_patch: str,
    head_before: str | None,
    create_review_worktree: bool = False,
) -> PreparedPatch | dict[str, object]:
    try:
        return state.git.prepare_patch(
            request_id=request_id,
            base_sha=base_sha,
            raw_patch=raw_patch,
            create_review_worktree=create_review_worktree,
        )
    except PatchApplyError as exc:
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="conflict",
                accepted=False,
                comments=[_conflict_comment(str(exc), base_sha)],
                head_before=head_before,
            ),
        )
        return updated.to_dict(include_raw_patch=True)


async def _review_patch(  # noqa: PLR0913
    *,
    payload: PatchRequestBody,
    state: AppState,
    request_id: str,
    target_ref: str,
    base_sha: str | None,
    analysis: PatchAnalysis,
    prepared: PreparedPatch,
    head_before: str | None,
) -> ReviewDecision | dict[str, object]:
    review_worktree = prepared.review_worktree
    if review_worktree is None:
        msg = "review worktree was not prepared"
        raise RuntimeError(msg)
    context = ReviewContext(
        request_id=request_id,
        target_ref=target_ref,
        base_sha=base_sha,
        service_worktree_path=str(review_worktree),
        agent_worktree_path=_agent_visible_review_path(state, review_worktree),
        commit_message=payload.commit_message,
        author=payload.author,
        requester=payload.requester,
        argument=payload.argument,
        analysis=analysis,
    )
    try:
        raw = await state.reviewer.review(context)
        return validate_reviewer_response(raw, analysis)
    except (ReviewerError, ReviewerResponseError) as exc:
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="review_error",
                accepted=False,
                comments=[
                    _general_comment(
                        "Reviewer response was invalid or unavailable; GitAgent failed "
                        "closed and did not accept the patch.",
                        code="review_error",
                        detail=str(exc),
                    ),
                ],
                head_before=head_before,
            ),
        )
        return updated.to_dict(include_raw_patch=True)


def _agent_visible_review_path(state: AppState, review_worktree: Path) -> str:
    data_path = state.settings.data_path.resolve()
    resolved_worktree = review_worktree.resolve()
    try:
        relative = resolved_worktree.relative_to(data_path)
    except ValueError:
        return str(resolved_worktree)
    return str(state.settings.review_workspace_mount_path / relative)


def _run_validation_if_configured(  # noqa: PLR0913
    *,
    state: AppState,
    request_id: str,
    prepared: PreparedPatch,
    head_before: str | None,
    reviewer_session_id: str | None,
    reviewer_summary: str | None,
) -> dict[str, object] | None:
    command = state.settings.validation_command
    if command is None:
        return None
    try:
        validation = state.git.run_validation(
            prepared=prepared,
            command=command,
            timeout_seconds=state.settings.validation_timeout_seconds,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        updated = state.store.update(
            request_id,
            PatchRequestUpdate(
                status="validation_failed",
                accepted=False,
                comments=[
                    _general_comment(
                        "Configured validation command could not run; patch was not "
                        "accepted.",
                        code="validation_error",
                        detail=str(exc),
                    ),
                ],
                head_before=head_before,
                reviewer_session_id=reviewer_session_id,
                reviewer_summary=reviewer_summary,
            ),
        )
        return updated.to_dict(include_raw_patch=True)
    if validation.ok:
        return None
    updated = state.store.update(
        request_id,
        PatchRequestUpdate(
            status="validation_failed",
            accepted=False,
            comments=[
                _general_comment(
                    "Configured validation command failed; fix the errors, rebase if "
                    "needed, and resubmit.",
                    code="validation_failed",
                    detail=(validation.stderr or validation.stdout)[-4000:],
                ),
            ],
            head_before=head_before,
            reviewer_session_id=reviewer_session_id,
            reviewer_summary=reviewer_summary,
        ),
    )
    return updated.to_dict(include_raw_patch=True)


def _general_comment(
    message: str,
    *,
    code: str,
    detail: str | None = None,
) -> dict[str, object]:
    comment: dict[str, object] = {"side": "general", "message": message, "code": code}
    if detail:
        comment["detail"] = detail
    return comment


def _stale_comment(
    target_ref: str,
    base_sha: str | None,
    head_before: str | None,
) -> dict[str, object]:
    latest = head_before or NULL_SHA
    return _general_comment(
        f"{target_ref} is at {latest}, but the request base_sha is {base_sha}. "
        "Fetch the latest refs, rebase your work on the latest commit, regenerate "
        "the patch, and resubmit. GitAgent does not auto-rebase patches.",
        code="stale_base",
    )


def _conflict_comment(detail: str, base_sha: str | None) -> dict[str, object]:
    return _general_comment(
        f"Patch does not apply cleanly to base {base_sha or NULL_SHA}. Fetch latest, "
        "rebase your work, regenerate the patch, and resubmit.",
        code="conflict",
        detail=detail[-4000:] if detail else None,
    )


app = create_app()
