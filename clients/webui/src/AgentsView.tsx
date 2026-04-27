import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { Agent } from "./types";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { withRequiredEnvKeys } from "./envPrefill";
import {
    queryKeys,
    useAgents,
    useConnections,
    useHarnesses,
    useSessions,
    useSkills,
} from "./queries";
import { useErrorContext } from "./ErrorContext";

type AgentsViewProps = {
    onSessionCreated: (sessionId: string) => void;
};

const DEFAULT_HARNESS = "copilot-cli";

type AgentFormState = {
    agent_id: string;
    name: string;
    harness: string;
    system_prompt: string;
    skills: string[];
    env_vars: string;
    connection_id: string | null;
};

function emptyAgentForm(harnesses: string[]): AgentFormState {
    return {
        agent_id: "",
        name: "",
        harness: getInitialHarness(harnesses),
        system_prompt: "",
        skills: [],
        env_vars: "",
        connection_id: null,
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

export default function AgentsView({ onSessionCreated }: AgentsViewProps) {
    const { data: agents = [] } = useAgents();
    const { data: skills = [] } = useSkills();
    const { data: harnesses = [] } = useHarnesses();
    const { data: connections = [] } = useConnections();
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

    function startEditingAgent(agent: Agent) {
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
            },
        });
        stopEditingAgent();
    }

    return (
        <div className="view-content">
            <div className="view-header">
                <h2>Agents</h2>
                <button onClick={() => { setShowForm(!showForm); if (showForm) setEnvDirty(false); }} type="button">
                    {showForm ? "Cancel" : "New Agent"}
                </button>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={(e) => { void handleSubmit(e); }}>
                    <label>
                        Agent ID
                        <input
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="support-bot"
                            required
                            value={form.agent_id}
                            onChange={(e) => setForm({ ...form, agent_id: e.target.value })}
                        />
                    </label>
                    <label>
                        Display Name
                        <input
                            placeholder="Support Bot"
                            required
                            value={form.name}
                            onChange={(e) => setForm({ ...form, name: e.target.value })}
                        />
                    </label>
                    <label>
                        Kernel
                        <select
                            value={form.harness}
                            onChange={(e) => setForm({ ...form, harness: e.target.value })}
                        >
                            {harnesses.map((harness) => (
                                <option key={harness} value={harness}>
                                    {formatHarnessLabel(harness)}
                                </option>
                            ))}
                        </select>
                    </label>
                    <label>
                        Connection
                        <select
                            value={form.connection_id ?? ""}
                            onChange={(e) => setForm({ ...form, connection_id: e.target.value || null })}
                        >
                            <option value="">None</option>
                            {connections.map((conn) => (
                                <option key={conn.connection_id} value={conn.connection_id}>
                                    {conn.name} ({conn.connection_id})
                                </option>
                            ))}
                        </select>
                    </label>
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
                                    <label className="checkbox-label" key={skill.skill_id}>
                                        <input
                                            checked={form.skills.includes(skill.skill_id)}
                                            onChange={() => toggleFormSkill(skill.skill_id)}
                                            type="checkbox"
                                        />
                                        {skill.skill_id}
                                    </label>
                                ))}
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
                    <button disabled={busy} type="submit">
                        Create Agent
                    </button>
                </form>
            )}

            <div className="card-grid">
                {agents.map((agent) => (
                    <div className="card" key={agent.agent_id}>
                        <div className="card-body">
                            <h3>{agent.name}</h3>
                            <div className="muted">{agent.agent_id}</div>
                            <div className="tag">{agent.harness}</div>
                            {agent.connection_id && (
                                <span className="tag">
                                    {connections.find((c) => c.connection_id === agent.connection_id)?.name
                                        ?? agent.connection_id}
                                </span>
                            )}
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
                                        <input
                                            required
                                            value={editForm.name}
                                            onChange={(e) =>
                                                setEditForm({ ...editForm, name: e.target.value })}
                                        />
                                    </label>
                                    <label>
                                        Kernel
                                        <select
                                            value={editForm.harness}
                                            onChange={(e) =>
                                                setEditForm({ ...editForm, harness: e.target.value })}
                                        >
                                            {harnesses.map((harness) => (
                                                <option key={harness} value={harness}>
                                                    {formatHarnessLabel(harness)}
                                                </option>
                                            ))}
                                        </select>
                                    </label>
                                    <label>
                                        Connection
                                        <select
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
                                        </select>
                                    </label>
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
                                                    <label className="checkbox-label" key={skill.skill_id}>
                                                        <input
                                                            checked={editForm.skills.includes(skill.skill_id)}
                                                            onChange={() => toggleEditSkill(skill.skill_id)}
                                                            type="checkbox"
                                                        />
                                                        {skill.skill_id}
                                                    </label>
                                                ))}
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
                                        <button className="small" disabled={busy} type="submit">
                                            Save
                                        </button>
                                        <button
                                            className="secondary-button small"
                                            onClick={stopEditingAgent}
                                            type="button"
                                        >
                                            Cancel
                                        </button>
                                    </div>
                                </form>
                            )}
                        </div>
                        <div className="card-footer">
                            <span className="muted">
                                Created {new Date(agent.created_at).toLocaleDateString()}
                            </span>
                            <div className="card-footer-actions">
                                <button
                                    className="small"
                                    disabled={busy}
                                    onClick={() => startSessionMutation.mutate(agent.agent_id)}
                                    type="button"
                                >
                                    New Session
                                </button>
                                {editingAgentId !== agent.agent_id && (
                                    <button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => startEditingAgent(agent)}
                                        type="button"
                                    >
                                        Edit
                                    </button>
                                )}
                                <button
                                    className="danger-button small"
                                    disabled={busy}
                                    onClick={() => deleteMutation.mutate(agent.agent_id)}
                                    type="button"
                                >
                                    Delete
                                </button>
                            </div>
                        </div>
                    </div>
                ))}
                {agents.length === 0 && (
                    <div className="empty-state">No agents yet. Create one to get started.</div>
                )}
            </div>
        </div>
    );
}
