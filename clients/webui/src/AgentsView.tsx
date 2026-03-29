import { FormEvent, useState } from "react";
import type { Agent, Skill } from "./types";

type AgentsViewProps = {
    agents: Agent[];
    skills: Skill[];
    onCreateAgent: (form: {
        agent_id: string;
        name: string;
        system_prompt: string;
        skills: string[];
    }) => Promise<void>;
    onUpdateAgent: (agentId: string, patch: { skills: string[] }) => Promise<void>;
    onDeleteAgent: (agentId: string) => Promise<void>;
    busy: boolean;
};

export default function AgentsView({
    agents,
    skills,
    onCreateAgent,
    onUpdateAgent,
    onDeleteAgent,
    busy,
}: AgentsViewProps) {
    const [form, setForm] = useState({
        agent_id: "",
        name: "",
        system_prompt: "",
        skills: [] as string[],
    });
    const [showForm, setShowForm] = useState(false);
    const [editingSkillsFor, setEditingSkillsFor] = useState<string | null>(null);
    const [editSkills, setEditSkills] = useState<string[]>([]);

    async function handleSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        await onCreateAgent(form);
        setForm({ agent_id: "", name: "", system_prompt: "", skills: [] });
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
        await onUpdateAgent(agentId, { skills: editSkills });
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
                <button onClick={() => setShowForm(!showForm)} type="button">
                    {showForm ? "Cancel" : "New Agent"}
                </button>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={handleSubmit}>
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
                        System Prompt
                        <textarea
                            placeholder="Optional system prompt"
                            rows={4}
                            value={form.system_prompt}
                            onChange={(e) => setForm({ ...form, system_prompt: e.target.value })}
                        />
                    </label>
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
                                            onClick={() => handleSaveSkills(agent.agent_id)}
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
                                    onClick={() => onDeleteAgent(agent.agent_id)}
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
