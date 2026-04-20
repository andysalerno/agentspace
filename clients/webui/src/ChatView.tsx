import type { FormEvent, KeyboardEvent } from "react";
import { useEffect, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { api } from "./api";
import type {
    ChatMessage,
    KernelEvent,
    MessageStreamFinalChunk,
    SessionDetail,
    ToolCall,
} from "./types";
import ToolDetailPane from "./ToolDetailPane";
import {
    queryKeys,
    useAgents,
    useSession,
    useSessions,
} from "./queries";
import { useErrorContext } from "./ErrorContext";

type ChatViewProps = {
    selectedSessionId: string | null;
    onSelectSession: (sessionId: string | null) => void;
};

const markdownPlugins = [remarkGfm, remarkBreaks];

function createLocalMessage(
    sessionId: string,
    role: "user" | "assistant",
    content: string,
): ChatMessage {
    return {
        message_id: `${role}-${crypto.randomUUID()}`,
        session_id: sessionId,
        role,
        content,
        created_at: new Date().toISOString(),
        tool_calls: [],
    };
}

function applyEventToAssistant(
    message: ChatMessage,
    event: KernelEvent,
): ChatMessage {
    if (event.type === "text_delta" && event.content) {
        return { ...message, content: `${message.content}${event.content}` };
    }

    if (event.type === "reasoning_delta" && event.content) {
        return {
            ...message,
            reasoning: `${message.reasoning ?? ""}${event.content}`,
        };
    }

    if (event.type === "tool_call" && event.tool) {
        const nextToolCalls = [
            ...(message.tool_calls ?? []),
            {
                tool: event.tool,
                input: event.input ? JSON.stringify(event.input, null, 2) : undefined,
            } satisfies ToolCall,
        ];
        return { ...message, tool_calls: nextToolCalls };
    }

    if (event.type === "tool_result" && event.tool && event.output != null) {
        const toolCalls = [...(message.tool_calls ?? [])];
        const toolIndex = toolCalls.findIndex(
            (toolCall) => toolCall.tool === event.tool && toolCall.output === undefined,
        );
        if (toolIndex >= 0) {
            const toolCall = toolCalls[toolIndex];
            toolCalls[toolIndex] = { ...toolCall, output: event.output };
            return { ...message, tool_calls: toolCalls };
        }
    }

    return message;
}

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

export default function ChatView({ selectedSessionId, onSelectSession }: ChatViewProps) {
    const { data: agents = [] } = useAgents();
    const { data: sessions = [] } = useSessions();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [messageDraft, setMessageDraft] = useState("");
    const [newSessionAgentId, setNewSessionAgentId] = useState("");
    const [newSessionChannelName, setNewSessionChannelName] = useState("");
    const [showNewSession, setShowNewSession] = useState(false);
    const [selectedToolCall, setSelectedToolCall] = useState<ToolCall | null>(null);

    // Streaming local state (true client state — not server-cached).
    const [streamingMessage, setStreamingMessage] = useState<ChatMessage | null>(null);
    const [streaming, setStreaming] = useState(false);
    // Pause session polling while streaming so background refetches can't
    // overwrite the optimistic user message we wrote into the cache.
    const { data: selectedSession = null } = useSession(selectedSessionId, {
        poll: !streaming,
    });
    const streamControllerRef = useRef<AbortController | null>(null);
    const streamingSessionIdRef = useRef<string | null>(null);

    const createSessionMutation = useMutation({
        mutationFn: (payload: { agent_id: string; channel_name: string | null }) =>
            api.createSession({
                agent_id: payload.agent_id,
                channel_name: payload.channel_name,
                client_type: "webui",
            }),
        onSuccess: (session) => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            onSelectSession(session.session_id);
        },
        onError: reportError,
    });

    const resetMutation = useMutation({
        mutationFn: (sessionId: string) => api.resetSession(sessionId),
        onSuccess: (_, sessionId) => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
            void queryClient.invalidateQueries({ queryKey: queryKeys.kernels });
        },
        onError: reportError,
    });

    useEffect(() => {
        if (!newSessionAgentId && agents.length > 0) {
            setNewSessionAgentId(agents[0].agent_id);
        }
    }, [agents, newSessionAgentId]);

    // Abort any in-flight stream when the selected session changes.
    useEffect(() => {
        if (
            streamingSessionIdRef.current !== null
            && streamingSessionIdRef.current !== selectedSessionId
        ) {
            streamControllerRef.current?.abort();
            streamControllerRef.current = null;
            streamingSessionIdRef.current = null;
            setStreamingMessage(null);
            setStreaming(false);
        }
    }, [selectedSessionId]);

    // Abort on unmount.
    useEffect(() => {
        return () => {
            streamControllerRef.current?.abort();
        };
    }, []);

    function appendMessageToCache(sessionId: string, message: ChatMessage) {
        queryClient.setQueryData<SessionDetail | undefined>(
            queryKeys.session(sessionId),
            (current) => {
                if (!current || current.session_id !== sessionId) return current;
                return { ...current, messages: [...current.messages, message] };
            },
        );
    }

    function applyFinalChunk(sessionId: string, chunk: MessageStreamFinalChunk) {
        queryClient.setQueryData<SessionDetail | undefined>(
            queryKeys.session(sessionId),
            (current) => {
                if (!current || current.session_id !== sessionId) return current;
                return {
                    ...current,
                    ...chunk.session,
                    messages: [...current.messages, chunk.assistant_message],
                };
            },
        );
        void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
        void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
        void queryClient.invalidateQueries({ queryKey: queryKeys.kernels });
    }

    async function handleCreateSession(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        if (!newSessionAgentId) return;
        await createSessionMutation.mutateAsync({
            agent_id: newSessionAgentId,
            channel_name: newSessionChannelName || null,
        });
        setNewSessionChannelName("");
        setShowNewSession(false);
    }

    function sendMessage(message: string) {
        if (!selectedSessionId) return;
        streamControllerRef.current?.abort();
        streamControllerRef.current = null;
        streamingSessionIdRef.current = null;

        const activeSessionId = selectedSessionId;
        const userMessage = createLocalMessage(activeSessionId, "user", message);
        const pendingAssistant = createLocalMessage(activeSessionId, "assistant", "");

        appendMessageToCache(activeSessionId, userMessage);
        setStreamingMessage(pendingAssistant);
        setStreaming(true);

        const controller = api.streamMessage(activeSessionId, message, {
            onEvent: (event) => {
                setStreamingMessage((current) => {
                    if (!current || current.session_id !== activeSessionId) {
                        return current;
                    }
                    return applyEventToAssistant(current, event);
                });
            },
            onFinal: (chunk) => {
                applyFinalChunk(activeSessionId, chunk);
                setStreamingMessage(null);
                setStreaming(false);
                streamControllerRef.current = null;
                streamingSessionIdRef.current = null;
            },
            onError: (err) => {
                setStreamingMessage(null);
                setStreaming(false);
                streamControllerRef.current = null;
                streamingSessionIdRef.current = null;
                void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
                void queryClient.invalidateQueries({
                    queryKey: queryKeys.session(activeSessionId),
                });
                reportError(err);
            },
        });
        streamControllerRef.current = controller;
        streamingSessionIdRef.current = activeSessionId;
    }

    function handleSendMessage(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        if (!messageDraft.trim()) return;
        const msg = messageDraft.trim();
        setMessageDraft("");
        sendMessage(msg);
    }

    function handleResetSession() {
        if (!selectedSessionId) return;
        streamControllerRef.current?.abort();
        streamControllerRef.current = null;
        streamingSessionIdRef.current = null;
        setStreamingMessage(null);
        setStreaming(false);
        resetMutation.mutate(selectedSessionId);
    }

    const busy = streaming || createSessionMutation.isPending || resetMutation.isPending;

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
                    <form className="compact-form" onSubmit={(e) => { void handleCreateSession(e); }}>
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
                                onClick={handleResetSession}
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
                                            handleSendMessage(e as unknown as FormEvent<HTMLFormElement>);
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
