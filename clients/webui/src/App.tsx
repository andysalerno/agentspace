import { FormEvent, useEffect, useState } from "react";
import { api } from "./api";
import type {
  Agent,
  Channel,
  KernelSummary,
  SessionDetail,
  SessionSummary,
} from "./types";

type AgentFormState = {
  agent_id: string;
  name: string;
  system_prompt: string;
};

const initialAgentForm: AgentFormState = {
  agent_id: "",
  name: "",
  system_prompt: "",
};

export default function App() {
  const [agents, setAgents] = useState<Agent[]>([]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [channels, setChannels] = useState<Channel[]>([]);
  const [kernels, setKernels] = useState<KernelSummary[]>([]);
  const [selectedSessionId, setSelectedSessionId] = useState<string | null>(null);
  const [selectedSession, setSelectedSession] = useState<SessionDetail | null>(null);
  const [agentForm, setAgentForm] = useState<AgentFormState>(initialAgentForm);
  const [newSessionAgentId, setNewSessionAgentId] = useState("");
  const [newSessionCwd, setNewSessionCwd] = useState("");
  const [messageDraft, setMessageDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function refreshOverview() {
    const [agentData, sessionData, channelData, kernelData] = await Promise.all([
      api.listAgents(),
      api.listSessions(),
      api.listChannels(),
      api.listKernels(),
    ]);
    setAgents(agentData);
    setSessions(sessionData);
    setChannels(channelData);
    setKernels(kernelData);
    if (!selectedSessionId && sessionData.length > 0) {
      setSelectedSessionId(sessionData[0].session_id);
    }
    if (!newSessionAgentId && agentData.length > 0) {
      setNewSessionAgentId(agentData[0].agent_id);
    }
  }

  async function refreshSelectedSession(sessionId: string) {
    const detail = await api.getSession(sessionId);
    setSelectedSession(detail);
  }

  useEffect(() => {
    refreshOverview().catch((err: Error) => setError(err.message));
  }, []);

  useEffect(() => {
    if (!selectedSessionId) {
      setSelectedSession(null);
      return;
    }
    refreshSelectedSession(selectedSessionId).catch((err: Error) => setError(err.message));
  }, [selectedSessionId]);

  async function handleCreateAgent(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const created = await api.createAgent(agentForm);
      setAgentForm(initialAgentForm);
      await refreshOverview();
      setNewSessionAgentId(created.agent_id);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleDeleteAgent(agentId: string) {
    setBusy(true);
    setError(null);
    try {
      await api.deleteAgent(agentId);
      if (selectedSession?.agent_id === agentId) {
        setSelectedSessionId(null);
      }
      await refreshOverview();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleCreateSession(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!newSessionAgentId) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const session = await api.createSession({
        agent_id: newSessionAgentId,
        cwd: newSessionCwd || null,
      });
      await refreshOverview();
      setSelectedSessionId(session.session_id);
      setNewSessionCwd("");
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleSendMessage(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!selectedSessionId || !messageDraft.trim()) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.sendMessage(selectedSessionId, messageDraft.trim());
      setMessageDraft("");
      await Promise.all([refreshOverview(), refreshSelectedSession(selectedSessionId)]);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function handleResetSession() {
    if (!selectedSessionId) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.resetSession(selectedSessionId);
      await Promise.all([refreshOverview(), refreshSelectedSession(selectedSessionId)]);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="app-shell">
      <header className="topbar">
        <div>
          <h1>AgentSpace</h1>
          <p className="subtitle">
            Agents, sessions, channels, and kernel activity in one place.
          </p>
        </div>
        <button className="secondary-button" onClick={() => refreshOverview()} type="button">
          Refresh
        </button>
      </header>
      {error ? <div className="error-banner">{error}</div> : null}
      <main className="dashboard">
        <aside className="left-column">
          <section className="panel">
            <div className="panel-heading">
              <h2>Agents</h2>
              <span>{agents.length}</span>
            </div>
            <form className="stack" onSubmit={handleCreateAgent}>
              <label>
                Agent ID
                <input
                  pattern="[a-z]+(?:-[a-z]+)*"
                  placeholder="support-bot"
                  required
                  value={agentForm.agent_id}
                  onChange={(event) =>
                    setAgentForm((current) => ({
                      ...current,
                      agent_id: event.target.value,
                    }))
                  }
                />
              </label>
              <label>
                Display Name
                <input
                  placeholder="Support Bot"
                  required
                  value={agentForm.name}
                  onChange={(event) =>
                    setAgentForm((current) => ({
                      ...current,
                      name: event.target.value,
                    }))
                  }
                />
              </label>
              <label>
                System Prompt
                <textarea
                  placeholder="Optional prompt"
                  rows={4}
                  value={agentForm.system_prompt}
                  onChange={(event) =>
                    setAgentForm((current) => ({
                      ...current,
                      system_prompt: event.target.value,
                    }))
                  }
                />
              </label>
              <button disabled={busy} type="submit">
                Add Agent
              </button>
            </form>
            <div className="list">
              {agents.map((agent) => (
                <div className="list-card" key={agent.agent_id}>
                  <div>
                    <strong>{agent.name}</strong>
                    <div className="muted">{agent.agent_id}</div>
                    <div className="muted">{agent.harness}</div>
                  </div>
                  <button
                    className="danger-button"
                    disabled={busy}
                    onClick={() => handleDeleteAgent(agent.agent_id)}
                    type="button"
                  >
                    Delete
                  </button>
                </div>
              ))}
            </div>
          </section>

          <section className="panel">
            <div className="panel-heading">
              <h2>Sessions</h2>
              <span>{sessions.length}</span>
            </div>
            <form className="stack compact" onSubmit={handleCreateSession}>
              <label>
                Agent
                <select
                  value={newSessionAgentId}
                  onChange={(event) => setNewSessionAgentId(event.target.value)}
                >
                  {agents.map((agent) => (
                    <option key={agent.agent_id} value={agent.agent_id}>
                      {agent.agent_id}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Working Directory
                <input
                  placeholder="/tmp/agent-session"
                  value={newSessionCwd}
                  onChange={(event) => setNewSessionCwd(event.target.value)}
                />
              </label>
              <button disabled={busy || !newSessionAgentId} type="submit">
                Start Chat Session
              </button>
            </form>
            <div className="list">
              {sessions.map((session) => {
                const mappedChannels = channels.filter(
                  (channel) => channel.session_id === session.session_id,
                );
                return (
                  <button
                    className={`session-card ${
                      selectedSessionId === session.session_id ? "active" : ""
                    }`}
                    key={session.session_id}
                    onClick={() => setSelectedSessionId(session.session_id)}
                    type="button"
                  >
                    <strong>{session.agent_id}</strong>
                    <div className="muted">{session.session_id}</div>
                    <div className="muted">
                      status: {session.status} | messages: {session.message_count}
                    </div>
                    {mappedChannels.length > 0 ? (
                      <div className="tag-row">
                        {mappedChannels.map((channel) => (
                          <span className="tag" key={channel.channel_id}>
                            channel: {channel.name}
                          </span>
                        ))}
                      </div>
                    ) : null}
                  </button>
                );
              })}
            </div>
          </section>
        </aside>

        <section className="chat-panel panel">
          <div className="panel-heading">
            <div>
              <h2>Chat</h2>
              <div className="muted">
                {selectedSession
                  ? `${selectedSession.agent_id} | ${selectedSession.session_id}`
                  : "Select a session"}
              </div>
            </div>
            <button
              className="secondary-button"
              disabled={busy || !selectedSessionId}
              onClick={handleResetSession}
              type="button"
            >
              Reset Session
            </button>
          </div>
          <div className="transcript">
            {selectedSession?.messages.length ? (
              selectedSession.messages.map((message) => (
                <article className={`message ${message.role}`} key={message.message_id}>
                  <header>{message.role}</header>
                  <div>{message.content}</div>
                </article>
              ))
            ) : (
              <div className="empty-state">No messages yet.</div>
            )}
          </div>
          <form className="composer" onSubmit={handleSendMessage}>
            <textarea
              placeholder="Send a message to the selected session"
              rows={5}
              value={messageDraft}
              onChange={(event) => setMessageDraft(event.target.value)}
            />
            <button disabled={busy || !selectedSessionId} type="submit">
              Send Message
            </button>
          </form>
        </section>

        <aside className="right-column">
          <section className="panel">
            <div className="panel-heading">
              <h2>Channels</h2>
              <span>{channels.length}</span>
            </div>
            <div className="list">
              {channels.length ? (
                channels.map((channel) => (
                  <div className="list-card" key={channel.channel_id}>
                    <div>
                      <strong>{channel.name}</strong>
                      <div className="muted">{channel.channel_type}</div>
                      <div className="muted">agent: {channel.agent_id}</div>
                      <div className="muted">session: {channel.session_id}</div>
                    </div>
                  </div>
                ))
              ) : (
                <div className="empty-state">No registered channels.</div>
              )}
            </div>
          </section>

          <section className="panel">
            <div className="panel-heading">
              <h2>Kernel Sessions</h2>
              <span>{kernels.length}</span>
            </div>
            <div className="list">
              {kernels.length ? (
                kernels.map((kernel) => (
                  <div className="list-card" key={kernel.session_id}>
                    <div>
                      <strong>{kernel.harness}</strong>
                      <div className="muted">kernel: {kernel.session_id}</div>
                      <div className="muted">status: {kernel.status}</div>
                      <div className="muted">client sessions: {kernel.client_session_ids.join(", ") || "none"}</div>
                      <div className="muted">channels: {kernel.channel_ids.join(", ") || "none"}</div>
                    </div>
                  </div>
                ))
              ) : (
                <div className="empty-state">No active kernels.</div>
              )}
            </div>
          </section>
        </aside>
      </main>
    </div>
  );
}
