import { FormEvent, useEffect, useState } from "react";
import type { Agent, SessionDetail, SessionSummary } from "./types";

type ChatViewProps = {
    agents: Agent[];
    sessions: SessionSummary[];
    selectedSessionId: string | null;
    selectedSession: SessionDetail | null;
    onSelectSession: (sessionId: string) => void;
    onCreateSession: (agentId: string, channelName: string) => Promise<void>;
    onSendMessage: (message: string) => Promise<void>;
    onResetSession: () => Promise<void>;
    busy: boolean;
};

export default function ChatView({
    agents,
    sessions,
    selectedSessionId,
    selectedSession,
    onSelectSession,
    onCreateSession,
    onSendMessage,
    onResetSession,
    busy,
}: ChatViewProps) {
    const [messageDraft, setMessageDraft] = useState("");
    const [newSessionAgentId, setNewSessionAgentId] = useState("");
    const [newSessionChannelName, setNewSessionChannelName] = useState("");
    const [showNewSession, setShowNewSession] = useState(false);

    useEffect(() => {
        if (!newSessionAgentId && agents.length > 0) {
            setNewSessionAgentId(agents[0].agent_id);
        }
    }, [agents, newSessionAgentId]);

    async function handleCreateSession(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        if (!newSessionAgentId) return;
        await onCreateSession(newSessionAgentId, newSessionChannelName);
        setNewSessionChannelName("");
        setShowNewSession(false);
    }

    async function handleSendMessage(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        if (!messageDraft.trim()) return;
        await onSendMessage(messageDraft.trim());
        setMessageDraft("");
    }

    return (
        <div className="chat-layout">
            <aside className="chat-sessions-panel">
                <div className="chat-sessions-heading">
                    <h3>Sessions</h3>
                    <button
                        className="icon-button"
                        onClick={() => setShowNewSession(!showNewSession)}
                        type="button"
                        title="New session"
                    >
                        {showNewSession ? "×" : "+"}
                    </button>
                </div>
                {showNewSession && (
                    <form className="compact-form" onSubmit={handleCreateSession}>
                        <select
                            value={newSessionAgentId}
                            onChange={(e) => setNewSessionAgentId(e.target.value)}
                        >
                            {agents.map((a) => (
                                <option key={a.agent_id} value={a.agent_id}>
                                    {a.name}
                                </option>
                            ))}
                        </select>
                        <input
                            placeholder="Channel name"
                            value={newSessionChannelName}
                            onChange={(e) => setNewSessionChannelName(e.target.value)}
                        />
                        <button disabled={busy || !newSessionAgentId} type="submit">
                            Start
                        </button>
                    </form>
                )}
                <div className="session-list">
                    {sessions.map((session) => (
                        <button
                            className={`session-item ${selectedSessionId === session.session_id ? "active" : ""}`}
                            key={session.session_id}
                            onClick={() => onSelectSession(session.session_id)}
                            type="button"
                        >
                            <strong>{session.agent_id}</strong>
                            <span className="muted">
                                {session.message_count} messages · {session.status}
                            </span>
                        </button>
                    ))}
                    {sessions.length === 0 && <div className="empty-state">No sessions yet</div>}
                </div>
            </aside>
            <section className="chat-main">
                {selectedSession ? (
                    <>
                        <div className="chat-header">
                            <div>
                                <h3>{selectedSession.agent_id}</h3>
                                <span className="muted">{selectedSession.session_id}</span>
                            </div>
                            <button
                                className="secondary-button"
                                disabled={busy}
                                onClick={onResetSession}
                                type="button"
                            >
                                Reset
                            </button>
                        </div>
                        <div className="transcript">
                            {selectedSession.messages.length > 0 ? (
                                selectedSession.messages.map((msg) => (
                                    <article className={`message ${msg.role}`} key={msg.message_id}>
                                        <header>{msg.role}</header>
                                        {msg.tool_calls && msg.tool_calls.length > 0 && (
                                            <div className="tool-calls">
                                                {msg.tool_calls.map((tc, i) => (
                                                    <span className="tool-call-tag" key={i}>
                                                        ⚙ {tc.tool}
                                                    </span>
                                                ))}
                                            </div>
                                        )}
                                        <div>{msg.content}</div>
                                    </article>
                                ))
                            ) : (
                                <div className="empty-state centered">
                                    Send a message to start the conversation.
                                </div>
                            )}
                        </div>
                        <form className="composer" onSubmit={handleSendMessage}>
                            <textarea
                                placeholder="Type a message…"
                                rows={3}
                                value={messageDraft}
                                onChange={(e) => setMessageDraft(e.target.value)}
                            />
                            <button disabled={busy || !messageDraft.trim()} type="submit">
                                Send
                            </button>
                        </form>
                    </>
                ) : (
                    <div className="empty-state centered full-height">
                        Select a session or create a new one to start chatting.
                    </div>
                )}
            </section>
        </div>
    );
}
