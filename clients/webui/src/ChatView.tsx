import { FormEvent, KeyboardEvent, useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import type { Agent, ChatMessage, SessionDetail, SessionSummary, ToolCall } from "./types";
import ToolDetailPane from "./ToolDetailPane";

type ChatViewProps = {
    agents: Agent[];
    sessions: SessionSummary[];
    selectedSessionId: string | null;
    selectedSession: SessionDetail | null;
    onSelectSession: (sessionId: string) => void;
    onCreateSession: (agentId: string, channelName: string) => Promise<void>;
    onSendMessage: (message: string) => void;
    onResetSession: () => Promise<void>;
    busy: boolean;
    streamingMessage: ChatMessage | null;
};

const markdownPlugins = [remarkGfm, remarkBreaks];

function MessageMarkdown({
    content,
    streaming = false,
}: {
    content: string;
    streaming?: boolean;
}) {
    return (
        <div className="message-content">
            <ReactMarkdown
                remarkPlugins={markdownPlugins}
                components={{
                    a: ({ href, children, ...props }) => (
                        <a
                            {...props}
                            href={href}
                            rel={href ? "noreferrer noopener" : undefined}
                            target={href ? "_blank" : undefined}
                        >
                            {children}
                        </a>
                    ),
                }}
            >
                {content}
            </ReactMarkdown>
            {streaming ? <span className="cursor">▌</span> : null}
        </div>
    );
}

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
    streamingMessage,
}: ChatViewProps) {
    const [messageDraft, setMessageDraft] = useState("");
    const [newSessionAgentId, setNewSessionAgentId] = useState("");
    const [newSessionChannelName, setNewSessionChannelName] = useState("");
    const [showNewSession, setShowNewSession] = useState(false);
    const [selectedToolCall, setSelectedToolCall] = useState<ToolCall | null>(null);

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
        const msg = messageDraft.trim();
        setMessageDraft("");
        onSendMessage(msg);
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
                            {selectedSession.messages.length > 0 || streamingMessage ? (
                                <>
                                    {selectedSession.messages.map((msg) => (
                                        <article className={`message ${msg.role}`} key={msg.message_id}>
                                            <header>{msg.role}</header>
                                            {msg.reasoning && (
                                                <details className="reasoning-block">
                                                    <summary>Reasoning</summary>
                                                    <div className="reasoning-content">{msg.reasoning}</div>
                                                </details>
                                            )}
                                            {msg.tool_calls && msg.tool_calls.length > 0 && (
                                                <div className="tool-calls">
                                                    {msg.tool_calls.map((tc, i) => (
                                                        <button
                                                            className="tool-call-tag"
                                                            key={i}
                                                            type="button"
                                                            onClick={() => setSelectedToolCall(tc)}
                                                        >
                                                            ⚙ {tc.tool}
                                                        </button>
                                                    ))}
                                                </div>
                                            )}
                                            <MessageMarkdown content={msg.content} />
                                        </article>
                                    ))}
                                    {streamingMessage && (
                                        <article
                                            className={`message ${streamingMessage.role} streaming`}
                                            key={streamingMessage.message_id}
                                        >
                                            <header>{streamingMessage.role}</header>
                                            {streamingMessage.reasoning && (
                                                <details className="reasoning-block" open>
                                                    <summary>Reasoning</summary>
                                                    <div className="reasoning-content">
                                                        {streamingMessage.reasoning}
                                                    </div>
                                                </details>
                                            )}
                                            {streamingMessage.tool_calls && streamingMessage.tool_calls.length > 0 && (
                                                <div className="tool-calls">
                                                    {streamingMessage.tool_calls.map((tc, i) => (
                                                        <button
                                                            className="tool-call-tag"
                                                            key={i}
                                                            type="button"
                                                            onClick={() => setSelectedToolCall(tc)}
                                                        >
                                                            ⚙ {tc.tool}
                                                        </button>
                                                    ))}
                                                </div>
                                            )}
                                            <MessageMarkdown content={streamingMessage.content} streaming />
                                        </article>
                                    )}
                                </>
                            ) : (
                                <div className="empty-state centered">
                                    Send a message to start the conversation.
                                </div>
                            )}
                        </div>
                        <form className="composer" onSubmit={handleSendMessage}>
                            <textarea
                                placeholder="Type a message…"
                                rows={1}
                                value={messageDraft}
                                onChange={(e) => setMessageDraft(e.target.value)}
                                onKeyDown={(e: KeyboardEvent<HTMLTextAreaElement>) => {
                                    if (e.key === "Enter" && !e.shiftKey) {
                                        e.preventDefault();
                                        if (messageDraft.trim() && !busy) {
                                            void handleSendMessage(e as unknown as FormEvent<HTMLFormElement>);
                                        }
                                    }
                                }}
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
            {selectedToolCall && (
                <ToolDetailPane
                    toolCall={selectedToolCall}
                    onClose={() => setSelectedToolCall(null)}
                />
            )}
        </div>
    );
}
