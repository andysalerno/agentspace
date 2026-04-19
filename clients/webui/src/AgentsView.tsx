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
    useHarnesses,
    useSkills,
} from "./queries";
import { useErrorContext } from "./ErrorContext";

type AgentsViewProps = {
    onSessionCreated: (sessionId: string) => void;
};

const DEFAULT_HARNESS = "copilot-cli";

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
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [form, setForm] = useState({
        agent_id: "",
        name: "",
        harness: getInitialHarness(harnesses),
        system_prompt: "",
        skills: [] as string[],
        env_vars: "",
    });
    const [showForm, setShowForm] = useState(false);
    const [editingSkillsFor, setEditingSkillsFor] = useState<string | null>(null);
    const [editSkills, setEditSkills] = useState<string[]>([]);
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
        }) => api.createAgent(payload),
        onSuccess: () => invalidateAgents(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({ agentId, patch }: { agentId: string; patch: { skills: string[] } }) =>
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
        setForm({
            agent_id: "",
            name: "",
            harness: getInitialHarness(harnesses),
            system_prompt: "",
            skills: [],
            env_vars: "",
        });
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
        setEditSkills((prev) =>
            prev.includes(skillId)
                ? prev.filter((s) => s !== skillId)
                : [...prev, skillId],
        );
    }

    async function handleSaveSkills(agentId: string) {
        await updateMutation.mutateAsync({ agentId, patch: { skills: editSkills } });
        setEditingSkillsFor(null);
    }

    function startEditingSkills(agent: Agent) {
        setEditingSkillsFor(agent.agent_id);
        setEditSkills([...agent.skills]);
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
                            {editingSkillsFor === agent.agent_id && (
                                <fieldset className="skills-fieldset">
                                    <legend>Edit Skills</legend>
                                    <div className="checkbox-grid">
                                        {skills.map((skill) => (
                                            <label className="checkbox-label" key={skill.skill_id}>
                                                <input
                                                    checked={editSkills.includes(skill.skill_id)}
                                                    onChange={() => toggleEditSkill(skill.skill_id)}
                                                    type="checkbox"
                                                />
                                                {skill.skill_id}
                                            </label>
                                        ))}
                                    </div>
                                    <div className="skills-edit-actions">
                                        <button
                                            className="small"
                                            disabled={busy}
                                            onClick={() => { void handleSaveSkills(agent.agent_id); }}
                                            type="button"
                                        >
                                            Save
                                        </button>
                                        <button
                                            className="secondary-button small"
                                            onClick={() => setEditingSkillsFor(null)}
                                            type="button"
                                        >
                                            Cancel
                                        </button>
                                    </div>
                                </fieldset>
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
                                {editingSkillsFor !== agent.agent_id && skills.length > 0 && (
                                    <button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => startEditingSkills(agent)}
                                        type="button"
                                    >
                                        Skills
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
