import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
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
import { useErrorContext } from "./ErrorContext";
import { Button, Checkbox, Combobox, Input, Option, Select } from "./fluent";

type AgentsViewProps = {
    onSessionCreated: (sessionId: string) => void;
};

const DEFAULT_HARNESS = "copilot-cli";
const DEFAULT_AGENT_SYSTEM_PROMPT =
    "You are a helpful assistant. Despite living inside a coding agent harness, you are not strictly a coding assistant. Instead, you help the user with any and all tasks they give you (possibly including coding!) using the tools and skills at your disposal. Pro tip: always prefer your skills and tools over generic CLI tools (though you can use those, too!)";

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

type WorkspaceMountFormState = {
    workspace_id: string;
    mode: WorkspaceMountMode;
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
    if (harnesses.includes(DEFAULT_HARNESS)) {
        return DEFAULT_HARNESS;
    }
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
            mount.workspace_id === workspaceId ? { ...mount, mode } : mount,
        );
    }
    return [...mounts, { workspace_id: workspaceId, mode }];
}

type ModelNameFieldProps = {
    connectionId: string | null;
    envVars: string;
    harness: string;
    onEnvVarsChange: (envVars: string) => void;
};

function modelIdsFromResponse(response: ConnectionModels | undefined): string[] {
    if (!response?.data) {
        return [];
    }
    return Array.from(
        new Set(
            response.data
                .map((model) => (typeof model === "string" ? model : model.id))
                .filter((id): id is string => typeof id === "string" && id.length > 0),
        ),
    );
}

function ModelNameField({
    connectionId,
    envVars,
    harness,
    onEnvVarsChange,
}: ModelNameFieldProps) {
    const modelKey = modelEnvKeyForHarness(harness);
    const modelsQuery = useConnectionModels(modelKey === null ? null : connectionId);
    if (modelKey === null) {
        return null;
    }

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
    const modelHelp = connectionId === null
        ? modelKey
        : modelsQuery.isLoading
            ? `${modelKey} · loading models...`
            : modelsQuery.isError
                ? `${modelKey} · model list unavailable`
                : modelIds.length > 0
                    ? `${modelKey} · ${modelIds.length} model${modelIds.length === 1 ? "" : "s"} available`
                    : `${modelKey} · no models returned`;
    const setModelName = (modelName: string) =>
        onEnvVarsChange(setEnvValue(envVars, modelKey, modelName));

    return (
        <label>
            Model
            <Combobox
                freeform
                inlinePopup
                placeholder={placeholder}
                selectedOptions={selectedOptions}
                value={value}
                onChange={(e) => setModelName(e.target.value)}
                onOptionSelect={(_, data) => {
                    const selectedModel = data.optionValue ?? data.optionText;
                    if (selectedModel) {
                        setModelName(selectedModel);
                    }
                }}
            >
                {visibleModelIds.map((modelId) => (
                    <Option key={modelId} text={modelId} value={modelId}>
                        {modelId}
                    </Option>
                ))}
                {connectionId !== null && modelsQuery.isLoading && (
                    <Option disabled text="Loading models">
                        Loading models...
                    </Option>
                )}
                {connectionId !== null && modelsQuery.isError && (
                    <Option disabled text="Model list unavailable">
                        Model list unavailable
                    </Option>
                )}
                {connectionId !== null
                    && !modelsQuery.isLoading
                    && !modelsQuery.isError
                    && modelIds.length > 0
                    && visibleModelIds.length === 0 && (
                    <Option disabled text="No matching models">
                        No matching models
                    </Option>
                )}
                {connectionId !== null
                    && !modelsQuery.isLoading
                    && !modelsQuery.isError
                    && modelIds.length === 0 && (
                    <Option disabled text="No models returned">
                        No models returned
                    </Option>
                )}
            </Combobox>
            <span className="muted">{modelHelp}</span>
        </label>
    );
}

export default function AgentsView({ onSessionCreated }: AgentsViewProps) {
    const { data: agents = [] } = useAgents();
    const { data: skills = [] } = useSkills();
    const { data: harnesses = [] } = useHarnesses();
    const { data: connections = [] } = useConnections();
    const { data: workspaces = [] } = useWorkspaces();
    const { data: sessions = [] } = useSessions();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [form, setForm] = useState<AgentFormState>(() => emptyAgentForm(harnesses));
    const [showForm, setShowForm] = useState(false);
    const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
    const [editForm, setEditForm] = useState<AgentFormState | null>(null);
    const [envDirty, setEnvDirty] = useState(false);

    const invalidateAgents = () =>
        queryClient.invalidateQueries({ queryKey: queryKeys.agents });

    const createMutation = useMutation({
        mutationFn: (payload: {
            agent_id: string;
            name: string;
            harness: string;
            system_prompt: string;
            skills: string[];
            env_vars: string;
            connection_id: string | null;
            workspace_mounts: WorkspaceMountFormState[];
        }) => api.createAgent(payload),
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
        }) =>
            api.updateAgent(agentId, patch),
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
            api.createSession({
                agent_id: agentId,
                channel_name: null,
                client_type: "webui",
            }),
        onSuccess: (session) => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            onSessionCreated(session.session_id);
        },
        onError: reportError,
    });

    const busy =
        createMutation.isPending
        || updateMutation.isPending
        || deleteMutation.isPending
        || startSessionMutation.isPending;

    useEffect(() => {
        if (harnesses.length === 0) {
            return;
        }
        if (!harnesses.includes(form.harness)) {
            setForm((prev) => ({ ...prev, harness: getInitialHarness(harnesses) }));
        }
    }, [form.harness, harnesses]);

    useEffect(() => {
        if (!showForm) return;
        if (envDirty) return;
        if (!form.harness) return;
        let cancelled = false;
        api.getKernelConfig(form.harness)
            .then((config) => {
                if (cancelled) return;
                setForm((prev) => ({
                    ...prev,
                    env_vars: withRequiredEnvKeys(config.env_vars, form.harness),
                }));
            })
            .catch(() => {
                // non-fatal: prefill is a convenience. Still surface required
                // keys so the user knows what to fill in.
                if (cancelled) return;
                setForm((prev) => ({
                    ...prev,
                    env_vars: withRequiredEnvKeys(prev.env_vars, form.harness),
                }));
            });
        return () => {
            cancelled = true;
        };
    }, [showForm, form.harness, envDirty]);

    async function handleSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        await createMutation.mutateAsync(form);
        setForm(emptyAgentForm(harnesses));
        setEnvDirty(false);
        setShowForm(false);
    }

    function toggleFormSkill(skillId: string) {
        setForm((prev) => ({
            ...prev,
            skills: prev.skills.includes(skillId)
                ? prev.skills.filter((s) => s !== skillId)
                : [...prev.skills, skillId],
        }));
    }

    function toggleEditSkill(skillId: string) {
        setEditForm((prev) => {
            if (prev === null) return prev;
            return {
                ...prev,
                skills: prev.skills.includes(skillId)
                    ? prev.skills.filter((s) => s !== skillId)
                    : [...prev.skills, skillId],
            };
        });
    }

    function setFormWorkspaceMode(workspaceId: string, mode: WorkspaceMountMode | "") {
        setForm((prev) => ({
            ...prev,
            workspace_mounts: mode
                ? upsertWorkspaceMount(prev.workspace_mounts, workspaceId, mode)
                : prev.workspace_mounts.filter((mount) => mount.workspace_id !== workspaceId),
        }));
    }

    function setEditWorkspaceMode(workspaceId: string, mode: WorkspaceMountMode | "") {
        setEditForm((prev) => {
            if (prev === null) return prev;
            return {
                ...prev,
                workspace_mounts: mode
                    ? upsertWorkspaceMount(prev.workspace_mounts, workspaceId, mode)
                    : prev.workspace_mounts.filter((mount) => mount.workspace_id !== workspaceId),
            };
        });
    }

    function startEditingAgent(agent: Agent) {
        setShowForm(false);
        setEnvDirty(false);
        setEditingAgentId(agent.agent_id);
        setEditForm(agentToForm(agent));
    }

    function stopEditingAgent() {
        setEditingAgentId(null);
        setEditForm(null);
    }

    function activeSessionCount(agentId: string) {
        return sessions.filter((session) => session.agent_id === agentId).length;
    }

    async function handleSaveAgent(agentId: string) {
        if (editForm === null) return;
        const activeCount = activeSessionCount(agentId);
        if (
            activeCount > 0
            && !window.confirm(
                `${activeCount} existing session${activeCount === 1 ? "" : "s"} use this agent. Save changes anyway? Those kernels need to be restarted before they pick up the new configuration.`,
            )
        ) {
            return;
        }
        await updateMutation.mutateAsync({
            agentId,
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
        stopEditingAgent();
    }

    return (
        <div className="view-content management-view agents-management-view">
            <div className="view-header">
                <div>
                    <h2>Agents</h2>
                    <span className="muted">
                        {agents.length} configured · {sessions.length} sessions · {skills.length} skills · {workspaces.length} workspaces
                    </span>
                </div>
                <div className="view-header-actions">
                    <Button onClick={() => { setShowForm(!showForm); stopEditingAgent(); if (showForm) setEnvDirty(false); }} type="button">
                        {showForm ? "Cancel" : "New Agent"}
                    </Button>
                </div>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={(e) => { void handleSubmit(e); }}>
                    <label>
                        Agent ID
                        <Input
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="support-bot"
                            required
                            value={form.agent_id}
                            onChange={(e) => setForm({ ...form, agent_id: e.target.value })}
                        />
                    </label>
                    <label>
                        Display Name
                        <Input
                            placeholder="Support Bot"
                            required
                            value={form.name}
                            onChange={(e) => setForm({ ...form, name: e.target.value })}
                        />
                    </label>
                    <label>
                        Kernel
                        <Select
                            value={form.harness}
                            onChange={(e) => setForm({ ...form, harness: e.target.value })}
                        >
                            {harnesses.map((harness) => (
                                <option key={harness} value={harness}>
                                    {formatHarnessLabel(harness)}
                                </option>
                            ))}
                        </Select>
                    </label>
                    <label>
                        Connection
                        <Select
                            value={form.connection_id ?? ""}
                            onChange={(e) => setForm({ ...form, connection_id: e.target.value || null })}
                        >
                            <option value="">None</option>
                            {connections.map((conn) => (
                                <option key={conn.connection_id} value={conn.connection_id}>
                                    {conn.name} ({conn.connection_id})
                                </option>
                            ))}
                        </Select>
                    </label>
                    <ModelNameField
                        connectionId={form.connection_id}
                        envVars={form.env_vars}
                        harness={form.harness}
                        onEnvVarsChange={(envVars) => {
                            setForm({ ...form, env_vars: envVars });
                            setEnvDirty(true);
                        }}
                    />
                    <div>
                        <label>System Prompt</label>
                        <CodeEditor
                            value={form.system_prompt}
                            onChange={(v) => setForm({ ...form, system_prompt: v })}
                            language="markdown"
                            height="120px"
                        />
                    </div>
                    {skills.length > 0 && (
                        <fieldset className="skills-fieldset">
                            <legend>Skills</legend>
                            <div className="checkbox-grid">
                                {skills.map((skill) => (
                                    <Checkbox
                                        checked={form.skills.includes(skill.skill_id)}
                                        className="checkbox-label"
                                        key={skill.skill_id}
                                        label={skill.skill_id}
                                        onChange={() => toggleFormSkill(skill.skill_id)}
                                    />
                                ))}
                            </div>
                        </fieldset>
                    )}
                    {workspaces.length > 0 && (
                        <fieldset className="skills-fieldset">
                            <legend>Workspaces</legend>
                            <span className="field-help">Mounted at /workspace/&lt;workspace-id&gt; when new sessions start.</span>
                            <div className="checkbox-grid">
                                {workspaces.map((workspace) => {
                                    const mount = form.workspace_mounts.find(
                                        (item) => item.workspace_id === workspace.workspace_id,
                                    );
                                    return (
                                        <label className="checkbox-label" key={workspace.workspace_id}>
                                            <span>{workspace.name} ({workspace.workspace_id})</span>
                                            <Select
                                                value={mount?.mode ?? ""}
                                                onChange={(e) =>
                                                    setFormWorkspaceMode(
                                                        workspace.workspace_id,
                                                        e.target.value as WorkspaceMountMode | "",
                                                    )}
                                            >
                                                <option value="">Not mounted</option>
                                                <option value="rw">Read/write</option>
                                                <option value="ro">Read-only</option>
                                            </Select>
                                        </label>
                                    );
                                })}
                            </div>
                        </fieldset>
                    )}
                    <div>
                        <label>Environment Variables</label>
                        <CodeEditor
                            value={form.env_vars}
                            onChange={(v) => { setForm({ ...form, env_vars: v }); setEnvDirty(true); }}
                            language="ini"
                            height="120px"
                        />
                        <span className="muted">Use .env file syntax: KEY=VALUE, one per line</span>
                    </div>
                    <Button disabled={busy} type="submit">
                        Create Agent
                    </Button>
                </form>
            )}

            <div className="card-grid management-card-grid">
                {agents.map((agent) => {
                    const sessionCount = activeSessionCount(agent.agent_id);
                    const connectionName = agent.connection_id
                        ? (connections.find((c) => c.connection_id === agent.connection_id)?.name
                            ?? agent.connection_id)
                        : "None";
                    return (
                    <div className="card management-card" key={agent.agent_id}>
                        <div className="card-body">
                            <div className="management-card-heading">
                                <div className="management-title-block">
                                    <h3>{agent.name}</h3>
                                    <code className="management-id">{agent.agent_id}</code>
                                </div>
                                <span className="tag">{agent.harness}</span>
                            </div>
                            <div className="card-meta management-meta">
                                <div>
                                    <strong>Connection</strong>
                                    <span className="truncate-value">{connectionName}</span>
                                </div>
                                 <div>
                                     <strong>Skills</strong>
                                     <span>{agent.skills.length}</span>
                                 </div>
                                 <div>
                                     <strong>Workspaces</strong>
                                     <span>{agent.workspace_mounts.length}</span>
                                 </div>
                                 <div>
                                     <strong>Sessions</strong>
                                     <span>{sessionCount}</span>
                                </div>
                            </div>
                            {agent.system_prompt && (
                                <p className="system-prompt-preview">{agent.system_prompt}</p>
                            )}
                             {agent.skills.length > 0 && (
                                 <div className="tag-row">
                                     {agent.skills.map((s) => (
                                        <span className="tag" key={s}>
                                            {s}
                                        </span>
                                     ))}
                                 </div>
                             )}
                             {agent.workspace_mounts.length > 0 && (
                                 <div className="tag-row">
                                     {agent.workspace_mounts.map((mount) => (
                                         <span className="tag" key={mount.workspace_id}>
                                             {mount.workspace_id}:{mount.mode}
                                         </span>
                                     ))}
                                 </div>
                             )}
                            {editingAgentId === agent.agent_id && editForm !== null && (
                                <form
                                    className="create-form agent-edit-form"
                                    onSubmit={(e) => {
                                        e.preventDefault();
                                        void handleSaveAgent(agent.agent_id);
                                    }}
                                >
                                    {activeSessionCount(agent.agent_id) > 0 && (
                                        <div className="warning-box">
                                            {activeSessionCount(agent.agent_id)} existing session{activeSessionCount(agent.agent_id) === 1 ? "" : "s"} use this agent. Save changes only after planning to restart those kernels.
                                        </div>
                                    )}
                                    <label>
                                        Display Name
                                        <Input
                                            required
                                            value={editForm.name}
                                            onChange={(e) =>
                                                setEditForm({ ...editForm, name: e.target.value })}
                                        />
                                    </label>
                                    <label>
                                        Kernel
                                        <Select
                                            value={editForm.harness}
                                            onChange={(e) =>
                                                setEditForm({ ...editForm, harness: e.target.value })}
                                        >
                                            {harnesses.map((harness) => (
                                                <option key={harness} value={harness}>
                                                    {formatHarnessLabel(harness)}
                                                </option>
                                            ))}
                                        </Select>
                                    </label>
                                    <label>
                                        Connection
                                        <Select
                                            value={editForm.connection_id ?? ""}
                                            onChange={(e) =>
                                                setEditForm({
                                                    ...editForm,
                                                    connection_id: e.target.value || null,
                                                })}
                                        >
                                            <option value="">None</option>
                                            {connections.map((conn) => (
                                                <option key={conn.connection_id} value={conn.connection_id}>
                                                    {conn.name} ({conn.connection_id})
                                                </option>
                                            ))}
                                        </Select>
                                    </label>
                                    <ModelNameField
                                        connectionId={editForm.connection_id}
                                        envVars={editForm.env_vars}
                                        harness={editForm.harness}
                                        onEnvVarsChange={(envVars) =>
                                            setEditForm({ ...editForm, env_vars: envVars })}
                                    />
                                    <div>
                                        <label>System Prompt</label>
                                        <CodeEditor
                                            value={editForm.system_prompt}
                                            onChange={(v) =>
                                                setEditForm({ ...editForm, system_prompt: v })}
                                            language="markdown"
                                            height="120px"
                                        />
                                    </div>
                                    {skills.length > 0 && (
                                        <fieldset className="skills-fieldset">
                                            <legend>Skills</legend>
                                            <div className="checkbox-grid">
                                                {skills.map((skill) => (
                                                    <Checkbox
                                                        checked={editForm.skills.includes(skill.skill_id)}
                                                        className="checkbox-label"
                                                        key={skill.skill_id}
                                                        label={skill.skill_id}
                                                        onChange={() => toggleEditSkill(skill.skill_id)}
                                                    />
                                                ))}
                                            </div>
                                        </fieldset>
                                    )}
                                    {workspaces.length > 0 && (
                                        <fieldset className="skills-fieldset">
                                            <legend>Workspaces</legend>
                                            <span className="field-help">Changes apply to new or restarted sessions.</span>
                                            <div className="checkbox-grid">
                                                {workspaces.map((workspace) => {
                                                    const mount = editForm.workspace_mounts.find(
                                                        (item) => item.workspace_id === workspace.workspace_id,
                                                    );
                                                    return (
                                                        <label className="checkbox-label" key={workspace.workspace_id}>
                                                            <span>{workspace.name} ({workspace.workspace_id})</span>
                                                            <Select
                                                                value={mount?.mode ?? ""}
                                                                onChange={(e) =>
                                                                    setEditWorkspaceMode(
                                                                        workspace.workspace_id,
                                                                        e.target.value as WorkspaceMountMode | "",
                                                                    )}
                                                            >
                                                                <option value="">Not mounted</option>
                                                                <option value="rw">Read/write</option>
                                                                <option value="ro">Read-only</option>
                                                            </Select>
                                                        </label>
                                                    );
                                                })}
                                            </div>
                                        </fieldset>
                                    )}
                                    <div>
                                        <label>Environment Variables</label>
                                        <CodeEditor
                                            value={editForm.env_vars}
                                            onChange={(v) => {
                                                setEditForm({ ...editForm, env_vars: v });
                                            }}
                                            language="ini"
                                            height="120px"
                                        />
                                        <span className="muted">Use .env file syntax: KEY=VALUE, one per line</span>
                                    </div>
                                    <div className="skills-edit-actions">
                                        <Button className="small" disabled={busy} type="submit">
                                            Save
                                        </Button>
                                        <Button
                                            className="secondary-button small"
                                            onClick={stopEditingAgent}
                                            type="button"
                                        >
                                            Cancel
                                        </Button>
                                    </div>
                                </form>
                            )}
                        </div>
                        <div className="card-footer">
                            <span className="muted">
                                Created {new Date(agent.created_at).toLocaleDateString()}
                            </span>
                            <div className="card-footer-actions">
                                <Button
                                    className="secondary-button small"
                                    onClick={() => {
                                        void api.downloadConfigResource(
                                            "agent",
                                            agent.agent_id,
                                        ).catch(reportError);
                                    }}
                                    type="button"
                                >
                                    Export YAML
                                </Button>
                                <Button
                                    className="small"
                                    disabled={busy}
                                    onClick={() => startSessionMutation.mutate(agent.agent_id)}
                                    type="button"
                                >
                                    New Session
                                </Button>
                                {editingAgentId !== agent.agent_id && (
                                    <Button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => startEditingAgent(agent)}
                                        type="button"
                                    >
                                        Edit
                                    </Button>
                                )}
                                <Button
                                    className="danger-button small"
                                    disabled={busy}
                                    onClick={() => deleteMutation.mutate(agent.agent_id)}
                                    type="button"
                                >
                                    Delete
                                </Button>
                            </div>
                        </div>
                    </div>
                    );
                })}
                {agents.length === 0 && (
                    <div className="empty-state">No agents yet. Create one to get started.</div>
                )}
            </div>
        </div>
    );
}
