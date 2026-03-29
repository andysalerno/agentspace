import { FormEvent, useState } from "react";
import type { Agent } from "./types";

type AgentsViewProps = {
  agents: Agent[];
  onCreateAgent: (form: {
    agent_id: string;
    name: string;
    system_prompt: string;
  }) => Promise<void>;
  onDeleteAgent: (agentId: string) => Promise<void>;
  busy: boolean;
};

export default function AgentsView({
  agents,
  onCreateAgent,
  onDeleteAgent,
  busy,
}: AgentsViewProps) {
  const [form, setForm] = useState({ agent_id: "", name: "", system_prompt: "" });
  const [showForm, setShowForm] = useState(false);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await onCreateAgent(form);
    setForm({ agent_id: "", name: "", system_prompt: "" });
    setShowForm(false);
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
            </div>
            <div className="card-footer">
              <span className="muted">
                Created {new Date(agent.created_at).toLocaleDateString()}
              </span>
              <button
                className="danger-button"
                disabled={busy}
                onClick={() => onDeleteAgent(agent.agent_id)}
                type="button"
              >
                Delete
              </button>
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
