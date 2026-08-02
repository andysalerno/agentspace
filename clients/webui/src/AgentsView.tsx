import { Fragment, useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
    Add20Regular,
    ArrowDownload20Regular,
    Bot24Regular,
    ChevronDown20Regular,
    ChevronRight20Regular,
    Delete20Regular,
    Edit20Regular,
    Play20Regular,
} from "@fluentui/react-icons";
import type { Agent, ConnectionModels, WorkspaceMountMode } from "./types";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import {
    getEnvValue,
    modelEnvKeyForHarness,
    setEnvValue,
    withRequiredEnvKeys,
} from "./envPrefill";
import {
    queryKeys,
    useAgents,
    useConnectionModels,
    useConnections,
    useHarnesses,
    useSessions,
    useSkills,
    useWorkspaces,
} from "./queries";
import { useErrorContext } from "./useErrorContext";
import {
    Button,
    Checkbox,
    Combobox,
    Field,
    Input,
    MessageBar,
    MessageBarBody,
    Option,
    Select,
} from "./fluent";
import { EmptyState, FormDialog, RowActions, ViewHeader } from "./ui";

type AgentsViewProps = {
    onSessionCreated: (sessionId: string) => void;
};

const DEFAULT_HARNESS = "copilot-cli";
const DEFAULT_AGENT_SYSTEM_PROMPT =
    "You are a helpful assistant. Despite living inside a coding agent harness, you are not strictly a coding assistant. Instead, you help the user with any and all tasks they give you (possibly including coding!) using the tools and skills at your disposal. Pro tip: always prefer your skills and tools over generic CLI tools (though you can use those, too!)";

type WorkspaceMountFormState = {
    workspace_id: string;
    mode: WorkspaceMountMode;
};

type AgentFormState = {
    agent_id: string;
    name: string;
    harness: string;
    system_prompt: string;
    skills: string[];
    env_vars: string;
    connection_id: string | null;
    workspace_mounts: WorkspaceMountFormState[];
};

function emptyAgentForm(harnesses: string[]): AgentFormState {
    return {
        agent_id: "",
        name: "",
        harness: getInitialHarness(harnesses),
        system_prompt: DEFAULT_AGENT_SYSTEM_PROMPT,
        skills: [],
        env_vars: "",
        connection_id: null,
        workspace_mounts: [],
    };
}

function agentToForm(agent: Agent): AgentFormState {
    return {
        agent_id: agent.agent_id,
        name: agent.name,
        harness: agent.harness,
        system_prompt: agent.system_prompt,
        skills: [...agent.skills],
        env_vars: agent.env_vars,
        connection_id: agent.connection_id,
        workspace_mounts: agent.workspace_mounts.map((mount) => ({
            workspace_id: mount.workspace_id,
            mode: mount.mode,
        })),
    };
}

function getInitialHarness(harnesses: string[]): string {
    if (harnesses.includes(DEFAULT_HARNESS)) return DEFAULT_HARNESS;
    return harnesses[0] ?? DEFAULT_HARNESS;
}

function formatHarnessLabel(harness: string): string {
    return harness
        .split("-")
        .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
        .join(" ");
}

function upsertWorkspaceMount(
    mounts: WorkspaceMountFormState[],
    workspaceId: string,
    mode: WorkspaceMountMode,
): WorkspaceMountFormState[] {
    if (mounts.some((mount) => mount.workspace_id === workspaceId)) {
        return mounts.map((mount) =>
            mount.workspace_id === workspaceId ? { ...mount, mode } : mount
        );
    }
    return [...mounts, { workspace_id: workspaceId, mode }];
}

function modelIdsFromResponse(response: ConnectionModels | undefined): string[] {
    if (!response?.data) return [];
    return Array.from(
        new Set(
            response.data
                .map((model) => (typeof model === "string" ? model : model.id))
                .filter((id): id is string => typeof id === "string" && id.length > 0),
        ),
    );
}

/**
 * Model picker backed by the selected connection's model list. Freeform, since
 * a connection may be unreachable or expose models it does not advertise.
 */
function ModelNameField(
    { connectionId, envVars, harness, onEnvVarsChange }: {
        connectionId: string | null;
        envVars: string;
        harness: string;
        onEnvVarsChange: (envVars: string) => void;
    },
) {
    const modelKey = modelEnvKeyForHarness(harness);
    const modelsQuery = useConnectionModels(modelKey === null ? null : connectionId);
    if (modelKey === null) return null;

    const modelIds = modelIdsFromResponse(modelsQuery.data);
    const value = getEnvValue(envVars, modelKey);
    const normalizedValue = value.trim().toLocaleLowerCase();
    const visibleModelIds = normalizedValue === ""
        ? modelIds
        : modelIds.filter((modelId) => modelId.toLocaleLowerCase().includes(normalizedValue));
    const selectedOptions = modelIds.includes(value) ? [value] : [];
    const placeholder = connectionId === null
        ? "Select a connection or type a model name"
        : modelsQuery.isError
        ? "Model list unavailable; type a model name"
        : "Select or type a model name";
    const hint = connectionId === null
        ? `Sets ${modelKey}.`
        : modelsQuery.isLoading
        ? `Sets ${modelKey}. Loading models…`
        : modelsQuery.isError
        ? `Sets ${modelKey}. Model list unavailable.`
        : modelIds.length > 0
        ? `Sets ${modelKey}. ${modelIds.length} model${
            modelIds.length === 1 ? "" : "s"
        } available.`
        : `Sets ${modelKey}. The connection returned no models.`;
    const setModelName = (modelName: string) =>
        onEnvVarsChange(setEnvValue(envVars, modelKey, modelName));

    return (
        <Field hint={hint} label="Model">
            <Combobox
                freeform
                inlinePopup
                onChange={(e) => setModelName(e.target.value)}
                onOptionSelect={(_, data) => {
                    const selectedModel = data.optionValue ?? data.optionText;
                    if (selectedModel) setModelName(selectedModel);
                }}
                placeholder={placeholder}
                selectedOptions={selectedOptions}
                value={value}
            >
                {visibleModelIds.map((modelId) => (
                    <Option key={modelId} text={modelId} value={modelId}>{modelId}</Option>
                ))}
                {connectionId !== null && modelsQuery.isLoading && (
                    <Option disabled text="Loading models">Loading models…</Option>
                )}
                {connectionId !== null && modelsQuery.isError && (
                    <Option disabled text="Model list unavailable">Model list unavailable</Option>
                )}
                {connectionId !== null && !modelsQuery.isLoading && !modelsQuery.isError
                    && modelIds.length > 0 && visibleModelIds.length === 0 && (
                    <Option disabled text="No matching models">No matching models</Option>
                )}
                {connectionId !== null && !modelsQuery.isLoading && !modelsQuery.isError
                    && modelIds.length === 0 && (
                    <Option disabled text="No models returned">No models returned</Option>
                )}
            </Combobox>
        </Field>
    );
}

/** Body of both the create and the edit dialog. */
function AgentFormFields(
    { form, onChange, idEditable }: {
        form: AgentFormState;
        onChange: (next: AgentFormState) => void;
        idEditable: boolean;
    },
) {
    const { data: skills = [] } = useSkills();
    const { data: harnesses = [] } = useHarnesses();
    const { data: connections = [] } = useConnections();
    const { data: workspaces = [] } = useWorkspaces();

    function toggleSkill(skillId: string) {
        onChange({
            ...form,
            skills: form.skills.includes(skillId)
                ? form.skills.filter((s) => s !== skillId)
                : [...form.skills, skillId],
        });
    }

    function setWorkspaceMode(workspaceId: string, mode: WorkspaceMountMode | "") {
        onChange({
            ...form,
            workspace_mounts: mode
                ? upsertWorkspaceMount(form.workspace_mounts, workspaceId, mode)
                : form.workspace_mounts.filter((mount) => mount.workspace_id !== workspaceId),
        });
    }

    return (
        <>
            <div className="form-grid">
                {idEditable && (
                    <Field label="Agent ID" required>
                        <Input
                            onChange={(e) => onChange({ ...form, agent_id: e.target.value })}
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="support-bot"
                            required
                            value={form.agent_id}
                        />
                    </Field>
                )}
                <Field label="Display name" required>
                    <Input
                        onChange={(e) => onChange({ ...form, name: e.target.value })}
                        placeholder="Support Bot"
                        required
                        value={form.name}
                    />
                </Field>
                <Field label="Kernel">
                    <Select
                        onChange={(e) => onChange({ ...form, harness: e.target.value })}
                        value={form.harness}
                    >
                        {harnesses.map((harness) => (
                            <option key={harness} value={harness}>
                                {formatHarnessLabel(harness)}
                            </option>
                        ))}
                    </Select>
                </Field>
                <Field label="Connection">
                    <Select
                        onChange={(e) =>
                            onChange({ ...form, connection_id: e.target.value || null })}
                        value={form.connection_id ?? ""}
                    >
                        <option value="">None</option>
                        {connections.map((conn) => (
                            <option key={conn.connection_id} value={conn.connection_id}>
                                {conn.name} ({conn.connection_id})
                            </option>
                        ))}
                    </Select>
                </Field>
                <ModelNameField
                    connectionId={form.connection_id}
                    envVars={form.env_vars}
                    harness={form.harness}
                    onEnvVarsChange={(envVars) => onChange({ ...form, env_vars: envVars })}
                />
            </div>

            <Field label="System prompt">
                <CodeEditor
                    height="140px"
                    language="markdown"
                    onChange={(v) => onChange({ ...form, system_prompt: v })}
                    value={form.system_prompt}
                />
            </Field>

            {skills.length > 0 && (
                <fieldset className="field-group">
                    <legend>Skills</legend>
                    <div className="checkbox-grid">
                        {skills.map((skill) => (
                            <Checkbox
                                checked={form.skills.includes(skill.skill_id)}
                                key={skill.skill_id}
                                label={skill.skill_id}
                                onChange={() => toggleSkill(skill.skill_id)}
                            />
                        ))}
                    </div>
                </fieldset>
            )}

            {workspaces.length > 0 && (
                <fieldset className="field-group">
                    <legend>Workspaces</legend>
                    <span className="field-group-help">
                        Mounted at /workspace/&lt;workspace-id&gt; when a session starts. Changes
                        apply to new or restarted sessions.
                    </span>
                    <div className="checkbox-grid">
                        {workspaces.map((workspace) => {
                            const mount = form.workspace_mounts.find(
                                (item) => item.workspace_id === workspace.workspace_id,
                            );
                            return (
                                <div className="mount-row" key={workspace.workspace_id}>
                                    <span title={workspace.workspace_id}>{workspace.name}</span>
                                    <Select
                                        aria-label={`Mount mode for ${workspace.name}`}
                                        onChange={(e) =>
                                            setWorkspaceMode(
                                                workspace.workspace_id,
                                                e.target.value as WorkspaceMountMode | "",
                                            )}
                                        value={mount?.mode ?? ""}
                                    >
                                        <option value="">Not mounted</option>
                                        <option value="rw">Read/write</option>
                                        <option value="ro">Read-only</option>
                                    </Select>
                                </div>
                            );
                        })}
                    </div>
                </fieldset>
            )}

            <Field hint="One KEY=VALUE per line, using .env syntax." label="Environment variables">
                <CodeEditor
                    height="140px"
                    language="ini"
                    onChange={(v) => onChange({ ...form, env_vars: v })}
                    value={form.env_vars}
                />
            </Field>
        </>
    );
}

export default function AgentsView({ onSessionCreated }: AgentsViewProps) {
    const { data: agents = [] } = useAgents();
    const { data: connections = [] } = useConnections();
    const { data: harnesses = [] } = useHarnesses();
    const { data: sessions = [] } = useSessions();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [createForm, setCreateForm] = useState<AgentFormState>(() => emptyAgentForm(harnesses));
    const [showForm, setShowForm] = useState(false);
    const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
    const [editForm, setEditForm] = useState<AgentFormState | null>(null);
    const [envDirty, setEnvDirty] = useState(false);
    const [expandedAgentId, setExpandedAgentId] = useState<string | null>(null);

    // Fall back to a valid harness while the harness list is loading or when
    // the previously selected harness is no longer available.
    const form: AgentFormState = harnesses.length === 0 || harnesses.includes(createForm.harness)
        ? createForm
        : { ...createForm, harness: getInitialHarness(harnesses) };

    const invalidateAgents = () => queryClient.invalidateQueries({ queryKey: queryKeys.agents });

    const createMutation = useMutation({
        mutationFn: (payload: AgentFormState) => api.createAgent(payload),
        onSuccess: () => invalidateAgents(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({ agentId, patch }: {
            agentId: string;
            patch: {
                name?: string;
                harness?: string;
                system_prompt?: string;
                skills?: string[];
                env_vars?: string;
                connection_id?: string | null;
                workspace_mounts?: WorkspaceMountFormState[];
            };
        }) => api.updateAgent(agentId, patch),
        onSuccess: () => invalidateAgents(),
        onError: reportError,
    });

    const deleteMutation = useMutation({
        mutationFn: (agentId: string) => api.deleteAgent(agentId),
        onSuccess: () => invalidateAgents(),
        onError: reportError,
    });

    const startSessionMutation = useMutation({
        mutationFn: (agentId: string) =>
            api.createSession({ agent_id: agentId, channel_name: null, client_type: "webui" }),
        onSuccess: (session) => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            onSessionCreated(session.session_id);
        },
        onError: reportError,
    });

    const busy = createMutation.isPending
        || updateMutation.isPending
        || deleteMutation.isPending
        || startSessionMutation.isPending;

    // Prefill the new agent's environment from the kernel defaults, until the
    // user edits the field themselves.
    useEffect(() => {
        if (!showForm) return;
        if (envDirty) return;
        if (!form.harness) return;
        let cancelled = false;
        api.getKernelConfig(form.harness)
            .then((config) => {
                if (cancelled) return;
                setCreateForm((prev) => ({
                    ...prev,
                    env_vars: withRequiredEnvKeys(config.env_vars, form.harness),
                }));
            })
            .catch(() => {
                // Non-fatal: prefill is a convenience. Still surface required
                // keys so the user knows what to fill in.
                if (cancelled) return;
                setCreateForm((prev) => ({
                    ...prev,
                    env_vars: withRequiredEnvKeys(prev.env_vars, form.harness),
                }));
            });
        return () => {
            cancelled = true;
        };
    }, [showForm, form.harness, envDirty]);

    function activeSessionCount(agentId: string) {
        return sessions.filter((session) => session.agent_id === agentId).length;
    }

    async function handleCreate() {
        await createMutation.mutateAsync(form);
        setCreateForm(emptyAgentForm(harnesses));
        setEnvDirty(false);
        setShowForm(false);
    }

    async function handleSaveAgent() {
        if (editForm === null || editingAgentId === null) return;
        const activeCount = activeSessionCount(editingAgentId);
        if (
            activeCount > 0
            && !window.confirm(
                `${activeCount} existing session${
                    activeCount === 1 ? "" : "s"
                } use this agent. Save changes anyway? Those kernels need to be restarted before they pick up the new configuration.`,
            )
        ) {
            return;
        }
        await updateMutation.mutateAsync({
            agentId: editingAgentId,
            patch: {
                name: editForm.name,
                harness: editForm.harness,
                system_prompt: editForm.system_prompt,
                skills: editForm.skills,
                env_vars: editForm.env_vars,
                connection_id: editForm.connection_id,
                workspace_mounts: editForm.workspace_mounts,
            },
        });
        setEditingAgentId(null);
        setEditForm(null);
    }

    function connectionLabel(agent: Agent) {
        if (agent.connection_id === null) return null;
        return connections.find((c) => c.connection_id === agent.connection_id)?.name
            ?? agent.connection_id;
    }

    return (
        <div className="view-content">
            <ViewHeader
                actions={
                    <Button
                        appearance="primary"
                        icon={<Add20Regular />}
                        onClick={() => setShowForm(true)}
                        type="button"
                    >
                        New agent
                    </Button>
                }
                description={`${agents.length} configured, ${sessions.length} active sessions`}
                title="Agents"
            />
            <div className="view-body">
                {agents.length === 0
                    ? (
                        <EmptyState
                            action={
                                <Button appearance="primary" onClick={() => setShowForm(true)}>
                                    New agent
                                </Button>
                            }
                            description="An agent binds a kernel, a model connection, skills, and workspaces into something you can start a session with."
                            icon={<Bot24Regular />}
                            title="No agents yet"
                        />
                    )
                    : (
                        <div className="table-container">
                            <div className="table-scroll">
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th aria-label="Expand" />
                                            <th>Agent</th>
                                            <th>Kernel</th>
                                            <th>Connection</th>
                                            <th className="num">Skills</th>
                                            <th className="num">Workspaces</th>
                                            <th className="num">Sessions</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {agents.map((agent) => {
                                            const expanded = expandedAgentId === agent.agent_id;
                                            const connection = connectionLabel(agent);
                                            return (
                                                <Fragment key={agent.agent_id}>
                                                    <tr
                                                        className={expanded ? "expanded" : undefined}
                                                    >
                                                        <td className="expand-toggle-cell">
                                                            <Button
                                                                appearance="subtle"
                                                                aria-expanded={expanded}
                                                                aria-label={expanded
                                                                    ? `Hide details for ${agent.name}`
                                                                    : `Show details for ${agent.name}`}
                                                                icon={expanded
                                                                    ? <ChevronDown20Regular />
                                                                    : <ChevronRight20Regular />}
                                                                onClick={() =>
                                                                    setExpandedAgentId(
                                                                        expanded
                                                                            ? null
                                                                            : agent.agent_id,
                                                                    )}
                                                                size="small"
                                                            />
                                                        </td>
                                                        <td>
                                                            <div className="cell-identity">
                                                                <span className="cell-identity-name">
                                                                    {agent.name}
                                                                </span>
                                                                <span className="cell-identity-id">
                                                                    {agent.agent_id}
                                                                </span>
                                                            </div>
                                                        </td>
                                                        <td className="nowrap">
                                                            {formatHarnessLabel(agent.harness)}
                                                        </td>
                                                        <td>
                                                            {connection === null
                                                                ? <span className="muted">None</span>
                                                                : connection}
                                                        </td>
                                                        <td className="num">
                                                            {agent.skills.length}
                                                        </td>
                                                        <td className="num">
                                                            {agent.workspace_mounts.length}
                                                        </td>
                                                        <td className="num">
                                                            {activeSessionCount(agent.agent_id)}
                                                        </td>
                                                        <td className="actions-cell">
                                                            <RowActions
                                                                items={[
                                                                    {
                                                                        key: "edit",
                                                                        label: "Edit agent",
                                                                        icon: <Edit20Regular />,
                                                                        disabled: busy,
                                                                        onClick: () => {
                                                                            setEditingAgentId(
                                                                                agent.agent_id,
                                                                            );
                                                                            setEditForm(
                                                                                agentToForm(agent),
                                                                            );
                                                                        },
                                                                    },
                                                                    {
                                                                        key: "export",
                                                                        label: "Export YAML",
                                                                        icon: (
                                                                            <ArrowDownload20Regular />
                                                                        ),
                                                                        onClick: () => {
                                                                            void api
                                                                                .downloadConfigResource(
                                                                                    "agent",
                                                                                    agent.agent_id,
                                                                                ).catch(reportError);
                                                                        },
                                                                    },
                                                                    {
                                                                        key: "delete",
                                                                        label: "Delete agent",
                                                                        icon: <Delete20Regular />,
                                                                        destructive: true,
                                                                        disabled: busy,
                                                                        onClick: () =>
                                                                            deleteMutation.mutate(
                                                                                agent.agent_id,
                                                                            ),
                                                                    },
                                                                ]}
                                                                primary={{
                                                                    key: "session",
                                                                    label: "New session",
                                                                    icon: <Play20Regular />,
                                                                    disabled: busy,
                                                                    onClick: () =>
                                                                        startSessionMutation.mutate(
                                                                            agent.agent_id,
                                                                        ),
                                                                }}
                                                            />
                                                        </td>
                                                    </tr>
                                                    {expanded && (
                                                        <tr className="detail-row">
                                                            <td colSpan={8}>
                                                                <div className="detail-block">
                                                                    <dl className="detail-list stacked">
                                                                        <dt>Skills</dt>
                                                                        <dd>
                                                                            {agent.skills.length
                                                                                    === 0
                                                                                ? (
                                                                                    <span className="muted">
                                                                                        None
                                                                                    </span>
                                                                                )
                                                                                : (
                                                                                    <span className="tag-row">
                                                                                        {agent.skills
                                                                                            .map((
                                                                                                s,
                                                                                            ) => (
                                                                                                <span
                                                                                                    className="tag"
                                                                                                    key={s}
                                                                                                >
                                                                                                    {s}
                                                                                                </span>
                                                                                            ))}
                                                                                    </span>
                                                                                )}
                                                                        </dd>
                                                                        <dt>Workspaces</dt>
                                                                        <dd>
                                                                            {agent.workspace_mounts
                                                                                    .length === 0
                                                                                ? (
                                                                                    <span className="muted">
                                                                                        None
                                                                                    </span>
                                                                                )
                                                                                : (
                                                                                    <span className="tag-row">
                                                                                        {agent
                                                                                            .workspace_mounts
                                                                                            .map((
                                                                                                mount,
                                                                                            ) => (
                                                                                                <span
                                                                                                    className="tag mono"
                                                                                                    key={mount
                                                                                                        .workspace_id}
                                                                                                >
                                                                                                    {mount
                                                                                                        .workspace_id}:{mount
                                                                                                        .mode}
                                                                                                </span>
                                                                                            ))}
                                                                                    </span>
                                                                                )}
                                                                        </dd>
                                                                        <dt>Created</dt>
                                                                        <dd>
                                                                            {new Date(
                                                                                agent.created_at,
                                                                            ).toLocaleString()}
                                                                        </dd>
                                                                    </dl>
                                                                    {agent.system_prompt !== "" && (
                                                                        <>
                                                                            <span className="detail-block-label">
                                                                                System prompt
                                                                            </span>
                                                                            <p className="quote-block">
                                                                                {agent.system_prompt}
                                                                            </p>
                                                                        </>
                                                                    )}
                                                                </div>
                                                            </td>
                                                        </tr>
                                                    )}
                                                </Fragment>
                                            );
                                        })}
                                    </tbody>
                                </table>
                            </div>
                        </div>
                    )}
            </div>

            <FormDialog
                busy={busy}
                onOpenChange={(open) => {
                    setShowForm(open);
                    if (!open) setEnvDirty(false);
                }}
                onSubmit={() => {
                    void handleCreate();
                }}
                open={showForm}
                submitLabel="Create agent"
                title="New agent"
                wide
            >
                <AgentFormFields
                    form={form}
                    idEditable
                    onChange={(next) => {
                        if (next.env_vars !== form.env_vars) setEnvDirty(true);
                        setCreateForm(next);
                    }}
                />
            </FormDialog>

            <FormDialog
                busy={busy}
                onOpenChange={(open) => {
                    if (!open) {
                        setEditingAgentId(null);
                        setEditForm(null);
                    }
                }}
                onSubmit={() => {
                    void handleSaveAgent();
                }}
                open={editForm !== null}
                submitLabel="Save changes"
                title={`Edit ${editingAgentId ?? ""}`}
                wide
            >
                {editForm !== null && (
                    <>
                        {editingAgentId !== null && activeSessionCount(editingAgentId) > 0 && (
                            <MessageBar intent="warning">
                                <MessageBarBody>
                                    {activeSessionCount(editingAgentId)} existing session
                                    {activeSessionCount(editingAgentId) === 1 ? "" : "s"}{" "}
                                    use this agent. Their kernels must be restarted before they pick
                                    up the new configuration.
                                </MessageBarBody>
                            </MessageBar>
                        )}
                        <AgentFormFields
                            form={editForm}
                            idEditable={false}
                            onChange={setEditForm}
                        />
                    </>
                )}
            </FormDialog>
        </div>
    );
}
