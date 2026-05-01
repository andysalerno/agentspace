import type { FormEvent } from "react";
import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { useErrorContext } from "./ErrorContext";
import {
    queryKeys,
    useGitAgentConfig,
    useGitAgentRequest,
    useGitAgentRequests,
    useGitAgentStatus,
} from "./queries";
import type {
    GitAgentConfig,
    GitAgentConfigUpdate,
    GitAgentPolicy,
    GitAgentRequestSummary,
    GitAgentRequestsResponse,
    GitAgentReviewerConfig,
    GitAgentReview,
    GitAgentReviewComment,
    GitAgentStatus,
} from "./types";

const DEFAULT_REVIEW_AGENT_ID = "git-agent";
const DEFAULT_DEFAULT_BRANCH = "main";
const DEFAULT_WIP_PREFIX = "wip/";

type ConfigFormState = {
    remote_url: string;
    patch_url: string;
    default_branch: string;
    validation_command: string;
    review_agent_id: string;
    allowed_refs: string;
    allowed_ref_prefixes: string;
    protected_refs: string;
    protected_ref_prefixes: string;
    unprotected_refs: string;
    unprotected_ref_prefixes: string;
    skip_review_ref_prefixes: string;
    skip_validation_ref_prefixes: string;
    reviewer_name: string;
    reviewer_harness: string;
    reviewer_connection_id: string;
    reviewer_skills: string;
    reviewer_system_prompt: string;
    reviewer_env_vars: string;
};

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function field(source: unknown, key: string): unknown {
    return isRecord(source) ? source[key] : undefined;
}

function firstString(...values: unknown[]): string {
    for (const value of values) {
        if (typeof value === "string") {
            return value;
        }
        if (typeof value === "number") {
            return String(value);
        }
    }
    return "";
}

function firstBoolean(...values: unknown[]): boolean | null {
    for (const value of values) {
        if (typeof value === "boolean") {
            return value;
        }
    }
    return null;
}

function parseList(text: string): string[] {
    return text
        .split(/[\n,]/)
        .map((item) => item.trim())
        .filter(Boolean);
}

function listFromOne(value: unknown): string[] {
    if (Array.isArray(value)) {
        return value
            .map((item) =>
                typeof item === "string" || typeof item === "number" ? String(item) : "",
            )
            .map((item) => item.trim())
            .filter(Boolean);
    }
    if (typeof value === "string") {
        return parseList(value);
    }
    return [];
}

function listFrom(...values: unknown[]): string[] {
    for (const value of values) {
        const parsed = listFromOne(value);
        if (parsed.length > 0) {
            return parsed;
        }
    }
    return [];
}

function listWithDefault(fallback: string[], ...values: unknown[]): string[] {
    const parsed = listFrom(...values);
    return parsed.length > 0 ? parsed : fallback;
}

function listText(values: string[]): string {
    return values.join("\n");
}

function formatDate(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

function shortSha(value: string): string {
    if (!value) return "—";
    return value.length > 12 ? `${value.slice(0, 12)}…` : value;
}

function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
}

function statusClass(label: string): string {
    const normalized = label.toLowerCase();
    if (
        normalized.includes("error")
        || normalized.includes("fail")
        || normalized.includes("invalid")
        || normalized.includes("reject")
        || normalized.includes("block")
        || normalized.includes("conflict")
        || normalized.includes("stale")
        || normalized.includes("unhealthy")
    ) {
        return "error";
    }
    if (
        normalized.includes("pending")
        || normalized.includes("review")
        || normalized.includes("queue")
        || normalized.includes("apply")
        || normalized.includes("start")
        || normalized.includes("busy")
    ) {
        return "busy";
    }
    if (
        normalized.includes("ok")
        || normalized.includes("healthy")
        || normalized.includes("ready")
        || normalized.includes("run")
        || normalized.includes("accept")
        || normalized.includes("commit")
        || normalized.includes("merge")
        || normalized.includes("active")
    ) {
        return "running";
    }
    return "stopped";
}

function repoRecord(status: GitAgentStatus | undefined): Record<string, unknown> | null {
    const candidate = status?.repo ?? status?.repository ?? field(status, "git_repo");
    return isRecord(candidate) ? candidate : null;
}

function reviewerFromConfig(config: GitAgentConfig | undefined): GitAgentReviewerConfig | null {
    const candidates = [
        config?.reviewer_agent,
        config?.review_agent,
        field(config, "reviewer"),
        field(config, "agent"),
        field(config, "reviewer_agent_config"),
    ];
    for (const candidate of candidates) {
        if (isRecord(candidate)) {
            return candidate as GitAgentReviewerConfig;
        }
    }
    return null;
}

function defaultBranch(config: GitAgentConfig | undefined, status: GitAgentStatus | undefined): string {
    const repo = repoRecord(status);
    return (
        firstString(
            config?.default_branch,
            field(config, "defaultBranch"),
            status?.default_branch,
            field(status, "defaultBranch"),
            repo?.default_branch,
            field(repo, "defaultBranch"),
        )
        || DEFAULT_DEFAULT_BRANCH
    );
}

function serviceStatus(status: GitAgentStatus | undefined): string {
    const label = firstString(
        status?.status,
        status?.service_status,
        status?.state,
        field(status, "health"),
    );
    if (label) return label;
    const healthy = firstBoolean(status?.healthy, field(status, "ok"));
    if (healthy === null) return "unknown";
    return healthy ? "running" : "error";
}

function policyObject(config: GitAgentConfig | undefined): unknown {
    return config?.policy ?? field(config, "policies");
}

function policyLists(config: GitAgentConfig | undefined, status: GitAgentStatus | undefined) {
    const policy = policyObject(config);
    const branch = defaultBranch(config, status);
    return {
        allowedRefs: listFrom(
            config?.allowed_refs,
            field(policy, "allowed_refs"),
            field(config, "allowed_target_refs"),
        ),
        allowedRefPrefixes: listFrom(
            config?.allowed_ref_prefixes,
            field(policy, "allowed_ref_prefixes"),
            field(config, "allowed_target_prefixes"),
        ),
        protectedRefs: listWithDefault(
            [branch],
            config?.protected_refs,
            field(policy, "protected_refs"),
            field(config, "protected_branches"),
        ),
        protectedRefPrefixes: listFrom(
            config?.protected_ref_prefixes,
            field(policy, "protected_ref_prefixes"),
        ),
        unprotectedRefs: listFrom(
            config?.unprotected_refs,
            field(policy, "unprotected_refs"),
        ),
        unprotectedRefPrefixes: listWithDefault(
            [DEFAULT_WIP_PREFIX],
            config?.unprotected_ref_prefixes,
            field(policy, "unprotected_ref_prefixes"),
        ),
        skipReviewRefPrefixes: listWithDefault(
            [DEFAULT_WIP_PREFIX],
            config?.skip_review_ref_prefixes,
            field(policy, "skip_review_ref_prefixes"),
            field(policy, "review_skipped_ref_prefixes"),
        ),
        skipValidationRefPrefixes: listWithDefault(
            [DEFAULT_WIP_PREFIX],
            config?.skip_validation_ref_prefixes,
            field(policy, "skip_validation_ref_prefixes"),
            field(policy, "validation_skipped_ref_prefixes"),
        ),
    };
}

function formFromConfig(
    config: GitAgentConfig | undefined,
    status: GitAgentStatus | undefined,
): ConfigFormState {
    const repo = repoRecord(status);
    const policy = policyLists(config, status);
    const reviewer = reviewerFromConfig(config);
    return {
        remote_url: firstString(
            config?.remote_url,
            field(config, "remoteUrl"),
            status?.remote_url,
            repo?.remote_url,
            field(repo, "remoteUrl"),
        ),
        patch_url: firstString(
            config?.patch_url,
            field(config, "patchUrl"),
            status?.patch_url,
            repo?.patch_url,
            field(repo, "patchUrl"),
        ),
        default_branch: defaultBranch(config, status),
        validation_command: firstString(
            config?.validation_command,
            field(config, "validate_command"),
            field(policyObject(config), "validation_command"),
        ),
        review_agent_id: firstString(
            config?.review_agent_id,
            config?.reviewer_agent_id,
            field(config, "reviewAgentId"),
            reviewer?.agent_id,
        ) || DEFAULT_REVIEW_AGENT_ID,
        allowed_refs: listText(policy.allowedRefs),
        allowed_ref_prefixes: listText(policy.allowedRefPrefixes),
        protected_refs: listText(policy.protectedRefs),
        protected_ref_prefixes: listText(policy.protectedRefPrefixes),
        unprotected_refs: listText(policy.unprotectedRefs),
        unprotected_ref_prefixes: listText(policy.unprotectedRefPrefixes),
        skip_review_ref_prefixes: listText(policy.skipReviewRefPrefixes),
        skip_validation_ref_prefixes: listText(policy.skipValidationRefPrefixes),
        reviewer_name: firstString(reviewer?.name, field(config, "reviewer_name")),
        reviewer_harness: firstString(reviewer?.harness, field(config, "reviewer_harness")),
        reviewer_connection_id: firstString(
            reviewer?.connection_id,
            field(config, "reviewer_connection_id"),
        ),
        reviewer_skills: listText(listFrom(reviewer?.skills, field(config, "reviewer_skills"))),
        reviewer_system_prompt: firstString(
            reviewer?.system_prompt,
            field(config, "reviewer_system_prompt"),
        ),
        reviewer_env_vars: firstString(reviewer?.env_vars, field(config, "reviewer_env_vars")),
    };
}

function formToPayload(form: ConfigFormState): GitAgentConfigUpdate {
    const allowedRefs = parseList(form.allowed_refs);
    const allowedRefPrefixes = parseList(form.allowed_ref_prefixes);
    const protectedRefs = parseList(form.protected_refs);
    const protectedRefPrefixes = parseList(form.protected_ref_prefixes);
    const unprotectedRefs = parseList(form.unprotected_refs);
    const unprotectedRefPrefixes = parseList(form.unprotected_ref_prefixes);
    const skipReviewRefPrefixes = parseList(form.skip_review_ref_prefixes);
    const skipValidationRefPrefixes = parseList(form.skip_validation_ref_prefixes);
    const policy: GitAgentPolicy = {
        allowed_refs: allowedRefs,
        allowed_ref_prefixes: allowedRefPrefixes,
        protected_refs: protectedRefs,
        protected_ref_prefixes: protectedRefPrefixes,
        unprotected_refs: unprotectedRefs,
        unprotected_ref_prefixes: unprotectedRefPrefixes,
        skip_review_ref_prefixes: skipReviewRefPrefixes,
        skip_validation_ref_prefixes: skipValidationRefPrefixes,
    };
    return {
        remote_url: form.remote_url.trim(),
        patch_url: form.patch_url.trim(),
        default_branch: form.default_branch.trim() || DEFAULT_DEFAULT_BRANCH,
        validation_command: form.validation_command.trim(),
        review_agent_id: form.review_agent_id.trim() || DEFAULT_REVIEW_AGENT_ID,
        allowed_refs: allowedRefs,
        allowed_ref_prefixes: allowedRefPrefixes,
        protected_refs: protectedRefs,
        protected_ref_prefixes: protectedRefPrefixes,
        unprotected_refs: unprotectedRefs,
        unprotected_ref_prefixes: unprotectedRefPrefixes,
        skip_review_ref_prefixes: skipReviewRefPrefixes,
        skip_validation_ref_prefixes: skipValidationRefPrefixes,
        policy,
        reviewer_agent: {
            agent_id: form.review_agent_id.trim() || DEFAULT_REVIEW_AGENT_ID,
            name: form.reviewer_name.trim(),
            harness: form.reviewer_harness.trim(),
            connection_id: form.reviewer_connection_id.trim() || null,
            skills: parseList(form.reviewer_skills),
            system_prompt: form.reviewer_system_prompt,
            env_vars: form.reviewer_env_vars,
        },
    };
}

function normalizeRequests(
    response: GitAgentRequestsResponse | undefined,
): GitAgentRequestSummary[] {
    if (!response) return [];
    if (Array.isArray(response)) return response;
    for (const candidate of [
        response.requests,
        response.patch_requests,
        response.items,
        response.data,
        field(response, "results"),
    ]) {
        if (Array.isArray(candidate)) {
            return candidate.filter(isRecord) as GitAgentRequestSummary[];
        }
    }
    return [];
}

function requestId(request: GitAgentRequestSummary): string {
    return firstString(request.request_id, request.id, field(request, "requestId"));
}

function requestStatus(request: GitAgentRequestSummary): string {
    return firstString(request.status, field(request, "state")) || "unknown";
}

function requestTargetRef(request: GitAgentRequestSummary): string {
    return firstString(request.target_ref, field(request, "targetRef"), field(request, "target")) || "—";
}

function requestBaseSha(request: GitAgentRequestSummary): string {
    return firstString(request.base_sha, field(request, "baseSha"), field(request, "base_commit"));
}

function requestHeadSha(request: GitAgentRequestSummary): string {
    return firstString(request.head_sha, field(request, "headSha"), field(request, "head_commit"));
}

function requestCommitSha(request: GitAgentRequestSummary): string {
    return firstString(request.commit_sha, field(request, "commitSha"), field(request, "commit"));
}

function reviewFromRequest(request: GitAgentRequestSummary): GitAgentReview | null {
    for (const candidate of [
        field(request, "review"),
        field(request, "reviewer"),
        field(request, "review_result"),
        field(request, "reviewer_result"),
    ]) {
        if (isRecord(candidate)) {
            return candidate as GitAgentReview;
        }
    }
    return null;
}

function reviewSummary(request: GitAgentRequestSummary): string {
    const review = reviewFromRequest(request);
    return firstString(
        request.reviewer_summary,
        request.review_summary,
        request.summary,
        review?.summary,
        field(request, "reviewerSummary"),
        field(request, "reviewSummary"),
    );
}

function commentsFromRequest(request: GitAgentRequestSummary): GitAgentReviewComment[] {
    const review = reviewFromRequest(request);
    for (const candidate of [
        review?.comments,
        field(request, "comments"),
        field(request, "review_comments"),
        field(request, "reviewer_comments"),
    ]) {
        if (Array.isArray(candidate)) {
            return candidate.filter(isRecord) as GitAgentReviewComment[];
        }
    }
    return [];
}

function patchFromRequest(request: GitAgentRequestSummary): string {
    return firstString(
        field(request, "raw_patch"),
        field(request, "patch"),
        field(request, "diff"),
        field(request, "unified_diff"),
        field(request, "rawPatch"),
        field(request, "raw_diff"),
    );
}

function requester(request: GitAgentRequestSummary): string {
    return firstString(
        request.requester,
        request.requester_agent_id,
        field(request, "agent_id"),
        field(request, "requesterAgentId"),
    ) || "—";
}

function MetaItem({
    label,
    value,
    title,
    className,
}: {
    label: string;
    value: string;
    title?: string;
    className?: string;
}) {
    return (
        <div className={className}>
            <strong>{label}</strong>
            <span className="truncate-value" title={title ?? value}>
                {value || "—"}
            </span>
        </div>
    );
}

function PolicyLine({ label, values }: { label: string; values: string[] }) {
    return (
        <div>
            <strong>{label}</strong>
            <div className="tag-row">
                {values.length > 0 ? (
                    values.map((value) => (
                        <span className="tag" key={value}>
                            {value}
                        </span>
                    ))
                ) : (
                    <span className="muted">not set</span>
                )}
            </div>
        </div>
    );
}

function RequestDetail({
    request,
    loading,
}: {
    request: GitAgentRequestSummary | null;
    loading: boolean;
}) {
    if (!request) {
        return (
            <div className="empty-state">
                Select a request to inspect reviewer comments and raw patch contents.
            </div>
        );
    }

    const id = requestId(request) || "unknown";
    const status = requestStatus(request);
    const comments = commentsFromRequest(request);
    const patch = patchFromRequest(request);
    const summary = reviewSummary(request);
    const baseSha = requestBaseSha(request);
    const headSha = requestHeadSha(request);
    const commitSha = requestCommitSha(request);

    return (
        <section className="card management-card git-agent-detail-card">
            <div className="card-body">
                <div className="management-card-heading">
                    <div className="management-title-block">
                        <h3>Request detail</h3>
                        <code className="management-id">{id}</code>
                    </div>
                    <div className="badge-row">
                        {loading && <span className="tag">loading</span>}
                        <span className={`status-badge ${statusClass(status)}`}>{status}</span>
                    </div>
                </div>
                <div className="card-meta management-meta">
                    <MetaItem label="Target Ref" value={requestTargetRef(request)} />
                    <MetaItem label="Requester" value={requester(request)} />
                    <MetaItem label="Base SHA" value={shortSha(baseSha)} title={baseSha} />
                    <MetaItem label="Head SHA" value={shortSha(headSha)} title={headSha} />
                    <MetaItem label="Commit SHA" value={shortSha(commitSha)} title={commitSha} />
                    <MetaItem
                        label="Updated"
                        value={firstString(request.updated_at, field(request, "updatedAt"))}
                    />
                </div>
                {summary && (
                    <p className="system-prompt-preview">
                        <strong>Reviewer summary:</strong> {summary}
                    </p>
                )}
                <div className="git-agent-comments">
                    <h4>Reviewer comments</h4>
                    {comments.length > 0 ? (
                        <ul className="plain-list">
                            {comments.map((comment, index) => {
                                const path = firstString(comment.path, field(comment, "file")) || "unknown";
                                const line = firstString(comment.line, field(comment, "line_number"));
                                const side = firstString(comment.side) || "new";
                                const severity = firstString(comment.severity) || "comment";
                                const message = firstString(comment.message, field(comment, "body"));
                                return (
                                    <li className="git-agent-comment" key={`${path}-${line}-${index}`}>
                                        <div className="badge-row">
                                            <span className={`status-badge ${statusClass(severity)}`}>
                                                {severity}
                                            </span>
                                            <code className="mono">
                                                {path}
                                                {line ? `:${line}` : ""} ({side})
                                            </code>
                                        </div>
                                        <p>{message || "—"}</p>
                                    </li>
                                );
                            })}
                        </ul>
                    ) : (
                        <p className="muted">No reviewer comments returned.</p>
                    )}
                </div>
                <div className="git-agent-patch-section">
                    <h4>Raw patch</h4>
                    <pre className="skill-file-content git-agent-patch-block"><code>{patch || "Patch not included in this response."}</code></pre>
                </div>
            </div>
        </section>
    );
}

function InlineError({ label, error }: { label: string; error: unknown }) {
    return (
        <div className="warning-box">
            {label}: {errorMessage(error)}
        </div>
    );
}

export default function GitAgentView() {
    const configQuery = useGitAgentConfig();
    const statusQuery = useGitAgentStatus();
    const requestsQuery = useGitAgentRequests();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();
    const [editingConfig, setEditingConfig] = useState(false);
    const [selectedRequestId, setSelectedRequestId] = useState<string | null>(null);
    const [configForm, setConfigForm] = useState<ConfigFormState>(() =>
        formFromConfig(undefined, undefined),
    );

    const requests = useMemo(
        () => normalizeRequests(requestsQuery.data),
        [requestsQuery.data],
    );
    const selectedSummary = selectedRequestId
        ? requests.find((request) => requestId(request) === selectedRequestId) ?? null
        : null;
    const requestDetailQuery = useGitAgentRequest(selectedRequestId);
    const selectedRequest = requestDetailQuery.data ?? selectedSummary;

    const config = configQuery.data;
    const status = statusQuery.data;
    const repo = repoRecord(status);
    const reviewer = reviewerFromConfig(config);
    const policies = policyLists(config, status);
    const branch = defaultBranch(config, status);
    const health = serviceStatus(status);
    const remoteUrl = firstString(
        config?.remote_url,
        status?.remote_url,
        repo?.remote_url,
        field(repo, "remoteUrl"),
    );
    const patchUrl = firstString(
        config?.patch_url,
        status?.patch_url,
        repo?.patch_url,
        field(repo, "patchUrl"),
    );
    const headSha = firstString(
        status?.head_sha,
        status?.commit_sha,
        repo?.head_sha,
        repo?.commit_sha,
        field(repo, "headSha"),
    );
    const reviewAgentId = firstString(
        config?.review_agent_id,
        config?.reviewer_agent_id,
        reviewer?.agent_id,
    ) || DEFAULT_REVIEW_AGENT_ID;
    const reviewerSkills = listFrom(reviewer?.skills, field(config, "reviewer_skills"));
    const reviewerEnvVars = firstString(reviewer?.env_vars, field(config, "reviewer_env_vars"));
    const reviewerEnvCount = reviewerEnvVars
        .split("\n")
        .filter((line) => line.trim() && !line.trim().startsWith("#"))
        .length;

    const saveConfigMutation = useMutation({
        mutationFn: (payload: GitAgentConfigUpdate) => api.updateGitAgentConfig(payload),
        onSuccess: () => {
            setEditingConfig(false);
            void queryClient.invalidateQueries({ queryKey: queryKeys.gitAgentConfig });
            void queryClient.invalidateQueries({ queryKey: queryKeys.gitAgentStatus });
        },
        onError: reportError,
    });

    function openConfigEditor() {
        setConfigForm(formFromConfig(config, status));
        setEditingConfig(true);
    }

    function cancelConfigEditor() {
        setEditingConfig(false);
        setConfigForm(formFromConfig(config, status));
    }

    async function handleConfigSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        await saveConfigMutation.mutateAsync(formToPayload(configForm));
    }

    function refresh() {
        void queryClient.invalidateQueries({ queryKey: queryKeys.gitAgentConfig });
        void queryClient.invalidateQueries({ queryKey: queryKeys.gitAgentStatus });
        void queryClient.invalidateQueries({ queryKey: queryKeys.gitAgentRequests });
        if (selectedRequestId) {
            void queryClient.invalidateQueries({
                queryKey: queryKeys.gitAgentRequest(selectedRequestId),
            });
        }
    }

    return (
        <div className="view-content management-view git-agent-management-view">
            <div className="view-header">
                <div>
                    <h2>Git Agent</h2>
                    <span className="muted">
                        {health} · {requests.length} patch request{requests.length === 1 ? "" : "s"}
                    </span>
                </div>
                <div className="view-header-actions">
                    <button className="secondary-button" onClick={refresh} type="button">
                        Refresh
                    </button>
                    <button
                        onClick={editingConfig ? cancelConfigEditor : openConfigEditor}
                        type="button"
                    >
                        {editingConfig ? "Cancel" : "Edit Config"}
                    </button>
                </div>
            </div>

            <p className="muted management-intro">
                GitAgent is the final authority for patch requests. Accepted changes are
                squash commits, <code>{branch}</code> is protected, and <code>wip/*</code>
                refs are unprotected with validation/review skipped by policy.
            </p>

            {configQuery.isError && (
                <InlineError label="Failed to load GitAgent config" error={configQuery.error} />
            )}
            {statusQuery.isError && (
                <InlineError label="Failed to load GitAgent status" error={statusQuery.error} />
            )}
            {requestsQuery.isError && (
                <InlineError label="Failed to load GitAgent request history" error={requestsQuery.error} />
            )}

            {editingConfig && (
                <form
                    className="create-form card git-agent-config-form"
                    onSubmit={(event) => {
                        void handleConfigSubmit(event);
                    }}
                >
                    <label>
                        Remote URL
                        <input
                            placeholder="http://gitagent:8004/repo.git"
                            value={configForm.remote_url}
                            onChange={(event) =>
                                setConfigForm({ ...configForm, remote_url: event.target.value })}
                        />
                    </label>
                    <label>
                        Patch URL
                        <input
                            placeholder="http://gitagent:8004/PatchRequest"
                            value={configForm.patch_url}
                            onChange={(event) =>
                                setConfigForm({ ...configForm, patch_url: event.target.value })}
                        />
                    </label>
                    <label>
                        Default Branch
                        <input
                            placeholder="main"
                            required
                            value={configForm.default_branch}
                            onChange={(event) =>
                                setConfigForm({ ...configForm, default_branch: event.target.value })}
                        />
                    </label>
                    <label>
                        Review Agent ID
                        <input
                            placeholder={DEFAULT_REVIEW_AGENT_ID}
                            value={configForm.review_agent_id}
                            onChange={(event) =>
                                setConfigForm({ ...configForm, review_agent_id: event.target.value })}
                        />
                    </label>
                    <label>
                        Validation Command
                        <input
                            placeholder="just validate"
                            value={configForm.validation_command}
                            onChange={(event) =>
                                setConfigForm({ ...configForm, validation_command: event.target.value })}
                        />
                    </label>
                    <fieldset className="skills-fieldset">
                        <legend>Target ref policy</legend>
                        <span className="field-help">
                            Enter comma- or newline-separated refs/prefixes. Main is protected;
                            wip/* is unprotected and skipped for review/validation by default.
                        </span>
                        <label>
                            Allowed refs
                            <textarea
                                rows={2}
                                value={configForm.allowed_refs}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, allowed_refs: event.target.value })}
                            />
                        </label>
                        <label>
                            Allowed ref prefixes
                            <textarea
                                rows={2}
                                value={configForm.allowed_ref_prefixes}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, allowed_ref_prefixes: event.target.value })}
                            />
                        </label>
                        <label>
                            Protected refs
                            <textarea
                                rows={2}
                                value={configForm.protected_refs}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, protected_refs: event.target.value })}
                            />
                        </label>
                        <label>
                            Protected ref prefixes
                            <textarea
                                rows={2}
                                value={configForm.protected_ref_prefixes}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, protected_ref_prefixes: event.target.value })}
                            />
                        </label>
                        <label>
                            Unprotected refs
                            <textarea
                                rows={2}
                                value={configForm.unprotected_refs}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, unprotected_refs: event.target.value })}
                            />
                        </label>
                        <label>
                            Unprotected ref prefixes
                            <textarea
                                rows={2}
                                value={configForm.unprotected_ref_prefixes}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, unprotected_ref_prefixes: event.target.value })}
                            />
                        </label>
                        <label>
                            Skip review prefixes
                            <textarea
                                rows={2}
                                value={configForm.skip_review_ref_prefixes}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, skip_review_ref_prefixes: event.target.value })}
                            />
                        </label>
                        <label>
                            Skip validation prefixes
                            <textarea
                                rows={2}
                                value={configForm.skip_validation_ref_prefixes}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, skip_validation_ref_prefixes: event.target.value })}
                            />
                        </label>
                    </fieldset>
                    <fieldset className="skills-fieldset">
                        <legend>Reviewer agent config</legend>
                        <label>
                            Display Name
                            <input
                                placeholder="Git Agent Reviewer"
                                value={configForm.reviewer_name}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, reviewer_name: event.target.value })}
                            />
                        </label>
                        <label>
                            Kernel
                            <input
                                placeholder="copilot-cli"
                                value={configForm.reviewer_harness}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, reviewer_harness: event.target.value })}
                            />
                        </label>
                        <label>
                            Connection ID
                            <input
                                placeholder="optional connection id"
                                value={configForm.reviewer_connection_id}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, reviewer_connection_id: event.target.value })}
                            />
                        </label>
                        <label>
                            Skills
                            <textarea
                                placeholder="comma or newline separated skill ids"
                                rows={2}
                                value={configForm.reviewer_skills}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, reviewer_skills: event.target.value })}
                            />
                        </label>
                        <label>
                            System Prompt
                            <textarea
                                rows={6}
                                value={configForm.reviewer_system_prompt}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, reviewer_system_prompt: event.target.value })}
                            />
                        </label>
                        <label>
                            Environment Variables
                            <textarea
                                placeholder="KEY=VALUE"
                                rows={5}
                                value={configForm.reviewer_env_vars}
                                onChange={(event) =>
                                    setConfigForm({ ...configForm, reviewer_env_vars: event.target.value })}
                            />
                        </label>
                    </fieldset>
                    <div className="skills-edit-actions">
                        <button disabled={saveConfigMutation.isPending} type="submit">
                            {saveConfigMutation.isPending ? "Saving…" : "Save Config"}
                        </button>
                        <button
                            className="secondary-button"
                            disabled={saveConfigMutation.isPending}
                            onClick={cancelConfigEditor}
                            type="button"
                        >
                            Cancel
                        </button>
                    </div>
                </form>
            )}

            <div className="card-grid management-card-grid git-agent-overview-grid">
                <div className="card management-card">
                    <div className="card-body">
                        <div className="management-card-heading">
                            <div className="management-title-block">
                                <h3>Service status</h3>
                                <code className="management-id">/api/git-agent/status</code>
                            </div>
                            <span className={`status-badge ${statusClass(health)}`}>{health}</span>
                        </div>
                        <div className="card-meta management-meta">
                            <MetaItem label="Repository" value={branch} />
                            <MetaItem label="Remote URL" value={remoteUrl} />
                            <MetaItem label="Patch URL" value={patchUrl} />
                            <MetaItem label="Head SHA" value={shortSha(headSha)} title={headSha} />
                            <MetaItem
                                label="Repo Empty"
                                value={String(
                                    firstBoolean(repo?.empty, field(status, "repo_empty")) ?? "unknown",
                                )}
                            />
                            <MetaItem
                                className={status?.last_error ? "error-text" : undefined}
                                label="Last Error"
                                value={status?.last_error ?? ""}
                            />
                        </div>
                    </div>
                </div>

                <div className="card management-card">
                    <div className="card-body">
                        <div className="management-card-heading">
                            <div className="management-title-block">
                                <h3>Ref policy</h3>
                                <code className="management-id">GitAgent final authority</code>
                            </div>
                            <span className="tag">squash commits</span>
                        </div>
                        <div className="git-agent-policy-list">
                            <PolicyLine label="Protected refs" values={policies.protectedRefs} />
                            <PolicyLine label="Protected prefixes" values={policies.protectedRefPrefixes} />
                            <PolicyLine label="Unprotected refs" values={policies.unprotectedRefs} />
                            <PolicyLine label="Unprotected prefixes" values={policies.unprotectedRefPrefixes} />
                            <PolicyLine label="Allowed refs" values={policies.allowedRefs} />
                            <PolicyLine label="Allowed prefixes" values={policies.allowedRefPrefixes} />
                            <PolicyLine
                                label="Skip review prefixes"
                                values={policies.skipReviewRefPrefixes}
                            />
                            <PolicyLine
                                label="Skip validation prefixes"
                                values={policies.skipValidationRefPrefixes}
                            />
                        </div>
                    </div>
                </div>

                <div className="card management-card">
                    <div className="card-body">
                        <div className="management-card-heading">
                            <div className="management-title-block">
                                <h3>Reviewer agent</h3>
                                <code className="management-id">{reviewAgentId}</code>
                            </div>
                            <span className="tag">{firstString(reviewer?.harness) || "unconfigured"}</span>
                        </div>
                        <div className="card-meta management-meta">
                            <MetaItem label="Name" value={firstString(reviewer?.name)} />
                            <MetaItem label="Harness" value={firstString(reviewer?.harness)} />
                            <MetaItem label="Connection" value={firstString(reviewer?.connection_id)} />
                            <MetaItem label="Skills" value={String(reviewerSkills.length)} />
                            <MetaItem label="Env Vars" value={String(reviewerEnvCount)} />
                            <MetaItem
                                label="Updated"
                                value={firstString(config?.updated_at, field(config, "updatedAt"))}
                            />
                        </div>
                        {firstString(reviewer?.system_prompt) && (
                            <p className="system-prompt-preview">
                                {firstString(reviewer?.system_prompt)}
                            </p>
                        )}
                        {reviewerSkills.length > 0 && (
                            <div className="tag-row">
                                {reviewerSkills.map((skill) => (
                                    <span className="tag" key={skill}>
                                        {skill}
                                    </span>
                                ))}
                            </div>
                        )}
                    </div>
                </div>
            </div>

            <section className="git-agent-requests-section">
                <div className="view-header git-agent-section-header">
                    <div>
                        <h3>Request history</h3>
                        <span className="muted">
                            Raw patches are available in the selected request detail.
                        </span>
                    </div>
                </div>
                {requests.length > 0 ? (
                    <div className="table-container management-table-container">
                        <table className="data-table management-table">
                            <thead>
                                <tr>
                                    <th>Status</th>
                                    <th>Request</th>
                                    <th>Target</th>
                                    <th>Requester</th>
                                    <th>Base</th>
                                    <th>Commit</th>
                                    <th>Reviewer Summary</th>
                                    <th>Updated</th>
                                    <th aria-label="Actions"></th>
                                </tr>
                            </thead>
                            <tbody>
                                {requests.map((request, index) => {
                                    const id = requestId(request);
                                    const statusLabel = requestStatus(request);
                                    const selected = id && selectedRequestId === id;
                                    const summary = reviewSummary(request);
                                    const baseSha = requestBaseSha(request);
                                    const commitSha = requestCommitSha(request);
                                    const updatedAt = firstString(
                                        request.updated_at,
                                        field(request, "updatedAt"),
                                        request.created_at,
                                        field(request, "createdAt"),
                                    );
                                    return (
                                        <tr key={id || index}>
                                            <td>
                                                <span className={`status-badge ${statusClass(statusLabel)}`}>
                                                    {statusLabel}
                                                </span>
                                            </td>
                                            <td className="mono" title={id}>
                                                <span className="truncate-value">{id || "—"}</span>
                                            </td>
                                            <td>
                                                <span className="truncate-value">
                                                    {requestTargetRef(request)}
                                                </span>
                                            </td>
                                            <td>
                                                <span className="truncate-value">{requester(request)}</span>
                                            </td>
                                            <td className="mono" title={baseSha}>
                                                {shortSha(baseSha)}
                                            </td>
                                            <td className="mono" title={commitSha}>
                                                {shortSha(commitSha)}
                                            </td>
                                            <td>
                                                <span className="truncate-value">{summary || "—"}</span>
                                            </td>
                                            <td className="nowrap">
                                                {updatedAt ? formatDate(updatedAt) : "—"}
                                            </td>
                                            <td className="actions-cell">
                                                <button
                                                    className="secondary-button small"
                                                    disabled={!id}
                                                    onClick={() => setSelectedRequestId(id)}
                                                    type="button"
                                                >
                                                    {selected ? "Selected" : "Details"}
                                                </button>
                                            </td>
                                        </tr>
                                    );
                                })}
                            </tbody>
                        </table>
                    </div>
                ) : (
                    <div className="empty-state">No patch requests yet.</div>
                )}
            </section>

            {requestDetailQuery.isError && (
                <InlineError label="Failed to load GitAgent request detail" error={requestDetailQuery.error} />
            )}
            <RequestDetail
                loading={requestDetailQuery.isFetching}
                request={selectedRequest ?? null}
            />
        </div>
    );
}
