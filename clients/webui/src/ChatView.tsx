import type { FormEvent, KeyboardEvent } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import { api } from "./api";
import { browserReachableLocalUrl } from "./browserUrls";
import type {
    AcpSessionUpdate,
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
    useKernels,
    useSession,
    useSessions,
} from "./queries";
import { useErrorContext } from "./ErrorContext";

type ChatViewProps = {
    selectedSessionId: string | null;
    onSelectSession: (sessionId: string | null) => void;
};

const markdownPlugins = [remarkGfm, remarkBreaks];
const toolCallHrefPrefix = "#tool-call-";

function createLocalMessage(
    sessionId: string,
    role: "user" | "assistant",
    content: string,
): ChatMessage {
    return {
        message_id: createClientMessageId(role),
        session_id: sessionId,
        role,
        content,
        created_at: new Date().toISOString(),
        tool_calls: [],
    };
}

function createClientMessageId(prefix: string): string {
    const cryptoObj = globalThis.crypto;
    if (typeof cryptoObj?.randomUUID === "function") {
        return `${prefix}-${cryptoObj.randomUUID()}`;
    }
    if (typeof cryptoObj?.getRandomValues === "function") {
        const bytes = new Uint8Array(16);
        cryptoObj.getRandomValues(bytes);
        const randomPart = Array.from(bytes, (byte) =>
            byte.toString(16).padStart(2, "0"),
        ).join("");
        return `${prefix}-${randomPart}`;
    }
    return `${prefix}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function applyEventToAssistant(
    message: ChatMessage,
    event: KernelEvent,
): ChatMessage {
    if (event.type === "session/update" && event.update) {
        return applyAcpUpdateToAssistant(message, event.update);
    }

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
                content_offset: message.content.trim().length,
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

function applyAcpUpdateToAssistant(
    message: ChatMessage,
    update: AcpSessionUpdate,
): ChatMessage {
    if (update.sessionUpdate === "agent_message_chunk") {
        return { ...message, content: `${message.content}${contentText(update.content)}` };
    }

    if (update.sessionUpdate === "agent_thought_chunk") {
        return {
            ...message,
            reasoning: `${message.reasoning ?? ""}${contentText(update.content)}`,
        };
    }

    if (update.sessionUpdate === "plan") {
        return {
            ...message,
            reasoning: `${message.reasoning ?? ""}${JSON.stringify({ plan: update.entries }, null, 2)}`,
        };
    }

    if (update.sessionUpdate === "tool_call" || update.sessionUpdate === "tool_call_update") {
        return upsertToolCall(message, update);
    }

    return message;
}

function upsertToolCall(message: ChatMessage, update: AcpSessionUpdate): ChatMessage {
    const toolCallId = typeof update.toolCallId === "string" ? update.toolCallId : undefined;
    const toolCalls = [...(message.tool_calls ?? [])];
    let toolIndex = toolCallId
        ? toolCalls.findIndex((toolCall) => toolCall.tool_call_id === toolCallId)
        : -1;

    if (toolIndex < 0) {
        toolCalls.push({
            tool: typeof update.title === "string" && update.title ? update.title : toolCallId ?? "tool",
            tool_call_id: toolCallId,
            content_offset: message.content.trim().length,
        });
        toolIndex = toolCalls.length - 1;
    }

    const current = toolCalls[toolIndex];
    toolCalls[toolIndex] = {
        ...current,
        tool: typeof update.title === "string" && update.title ? update.title : current.tool,
        status: typeof update.status === "string" ? update.status : current.status,
        kind: typeof update.kind === "string" ? update.kind : current.kind,
        input: Object.hasOwn(update, "rawInput") ? jsonText(update.rawInput) : current.input,
        output: toolOutput(update) ?? current.output,
    };
    return { ...message, tool_calls: toolCalls };
}

function toolOutput(update: AcpSessionUpdate): string | undefined {
    if (Object.hasOwn(update, "rawOutput")) {
        return jsonText(update.rawOutput);
    }
    const text = contentText(update.content);
    return text || undefined;
}

function jsonText(value: unknown): string | undefined {
    if (value == null) {
        return undefined;
    }
    return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

function contentText(content: unknown): string {
    if (Array.isArray(content)) {
        return content.map(contentText).join("");
    }
    if (content == null) {
        return "";
    }
    if (
        typeof content === "string" ||
        typeof content === "number" ||
        typeof content === "boolean" ||
        typeof content === "bigint"
    ) {
        return String(content);
    }
    if (typeof content === "symbol") {
        return content.description ?? "";
    }
    if (typeof content !== "object") {
        return "";
    }
    const block = content as Record<string, unknown>;
    if (block.type === "text") {
        return typeof block.text === "string" ? block.text : "";
    }
    if (block.type === "content") {
        return contentText(block.content);
    }
    return JSON.stringify(block);
}

function escapeMarkdownLinkText(value: string): string {
    return value.replace(/([\\[\]])/g, "\\$1");
}

function toolCallLink(toolCall: ToolCall, index: number): string {
    return `[⚙ ${escapeMarkdownLinkText(toolCall.tool)}](${toolCallHrefPrefix}${index})`;
}

function toolCallOffset(toolCall: ToolCall, contentLength: number): number {
    const offset = toolCall.content_offset;
    if (offset === undefined || !Number.isFinite(offset)) {
        return 0;
    }
    return Math.min(Math.max(Math.trunc(offset), 0), contentLength);
}

function addInlineToolCalls(content: string, toolCalls: ToolCall[]): string {
    if (toolCalls.length === 0) {
        return content;
    }

    const orderedToolCalls = toolCalls
        .map((toolCall, index) => ({
            index,
            offset: toolCallOffset(toolCall, content.length),
            toolCall,
        }))
        .sort((a, b) => a.offset - b.offset || a.index - b.index);
    let cursor = 0;
    let markdown = "";

    for (const { index, offset, toolCall } of orderedToolCalls) {
        markdown = `${markdown}${content.slice(cursor, offset)}`;
        const needsLeadingSpace = markdown.length > 0 && !/\s$/.test(markdown);
        const nextCharacter = content.slice(offset, offset + 1);
        const needsTrailingSpace = nextCharacter !== "" && !/\s/.test(nextCharacter);
        markdown = `${markdown}${needsLeadingSpace ? " " : ""}${toolCallLink(toolCall, index)}${needsTrailingSpace ? " " : ""}`;
        cursor = offset;
    }

    return `${markdown}${content.slice(cursor)}`;
}

function hasMessageWithId(messages: ChatMessage[], message: ChatMessage): boolean {
    return messages.some((existing) => existing.message_id === message.message_id);
}

function hasEquivalentServerMessage(
    messages: ChatMessage[],
    message: ChatMessage,
): boolean {
    return messages.some(
        (existing) =>
            existing.message_id !== message.message_id
            && existing.session_id === message.session_id
            && existing.role === message.role
            && existing.content === message.content,
    );
}

function MessageMarkdown({
    content,
    onSelectToolCall,
    streaming = false,
    toolCalls = [],
}: {
    content: string;
    onSelectToolCall?: (toolCall: ToolCall) => void;
    streaming?: boolean;
    toolCalls?: ToolCall[];
}) {
    const renderedContent = toolCalls.length > 0 ? content.trim() : content;
    const markdownContent = addInlineToolCalls(renderedContent, toolCalls);

    return (
        <div className="message-content">
            <ReactMarkdown
                remarkPlugins={markdownPlugins}
                components={{
                    a: ({ href, children, ...props }) => {
                        if (href?.startsWith(toolCallHrefPrefix)) {
                            const toolCallIndex = Number.parseInt(
                                href.slice(toolCallHrefPrefix.length),
                                10,
                            );
                            const toolCall = toolCalls[toolCallIndex];
                            if (toolCall) {
                                return (
                                    <button
                                        className="tool-call-tag inline-tool-call"
                                        type="button"
                                        onClick={() => onSelectToolCall?.(toolCall)}
                                    >
                                        {children}
                                    </button>
                                );
                            }
                        }
                        return (
                            <a
                                {...props}
                                href={href}
                                rel={href ? "noreferrer noopener" : undefined}
                                target={href ? "_blank" : undefined}
                            >
                                {children}
                            </a>
                        );
                    },
                }}
            >
                {markdownContent}
            </ReactMarkdown>
            {streaming ? <span className="cursor">▌</span> : null}
        </div>
    );
}

export default function ChatView({ selectedSessionId, onSelectSession }: ChatViewProps) {
    const { data: agents = [] } = useAgents();
    const { data: sessions = [] } = useSessions();
    const { data: kernels = [] } = useKernels();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [messageDraft, setMessageDraft] = useState("");
    const [newSessionAgentId, setNewSessionAgentId] = useState("");
    const [newSessionChannelName, setNewSessionChannelName] = useState("");
    const [showNewSession, setShowNewSession] = useState(false);
    const [selectedToolCall, setSelectedToolCall] = useState<ToolCall | null>(null);

    // Streaming local state (true client state — not server-cached).
    const [pendingUserMessage, setPendingUserMessage] = useState<ChatMessage | null>(null);
    const [streamingMessage, setStreamingMessage] = useState<ChatMessage | null>(null);
    const [streaming, setStreaming] = useState(false);
    // Pause session polling while streaming so background refetches can't
    // overwrite the optimistic user message we wrote into the cache.
    const { data: selectedSession = null } = useSession(selectedSessionId, {
        poll: !streaming,
    });
    const streamControllerRef = useRef<AbortController | null>(null);
    const streamingSessionIdRef = useRef<string | null>(null);
    const streamingTurnIdRef = useRef<string | null>(null);

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

    const deleteSessionMutation = useMutation({
        mutationFn: (sessionId: string) => api.deleteSession(sessionId),
        onSuccess: (_, sessionId) => {
            queryClient.removeQueries({ queryKey: queryKeys.session(sessionId) });
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            void queryClient.invalidateQueries({ queryKey: queryKeys.kernels });
            if (selectedSessionId === sessionId) {
                onSelectSession(null);
            }
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
            streamingTurnIdRef.current = null;
            setPendingUserMessage(null);
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

    const updateMessageInCache = useCallback((
        sessionId: string,
        messageId: string,
        updater: (message: ChatMessage) => ChatMessage,
    ) => {
        queryClient.setQueryData<SessionDetail | undefined>(
            queryKeys.session(sessionId),
            (current) => {
                if (!current || current.session_id !== sessionId) return current;
                return {
                    ...current,
                    messages: current.messages.map((message) => (
                        message.message_id === messageId ? updater(message) : message
                    )),
                };
            },
        );
    }, [queryClient]);

    const applyFinalChunk = useCallback((
        sessionId: string,
        chunk: MessageStreamFinalChunk,
        userMessage?: ChatMessage,
    ) => {
        queryClient.setQueryData<SessionDetail | undefined>(
            queryKeys.session(sessionId),
            (current) => {
                const messages = current?.session_id === sessionId
                    ? [...current.messages]
                    : [];
                if (userMessage && !hasMessageWithId(messages, userMessage)) {
                    messages.push(userMessage);
                }
                const assistantIndex = messages.findIndex(
                    (message) => message.message_id === chunk.assistant_message.message_id,
                );
                if (assistantIndex >= 0) {
                    messages[assistantIndex] = chunk.assistant_message;
                } else {
                    messages.push(chunk.assistant_message);
                }
                return {
                    ...current,
                    ...chunk.session,
                    messages,
                };
            },
        );
        void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
        void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
        void queryClient.invalidateQueries({ queryKey: queryKeys.kernels });
    }, [queryClient]);

    useEffect(() => {
        const activeTurn = selectedSession?.active_turn;
        if (!selectedSessionId || !activeTurn) return;
        if (streamingTurnIdRef.current === activeTurn.turn_id) return;
        if (
            streamControllerRef.current !== null
            && streamingSessionIdRef.current === selectedSessionId
            && streamingTurnIdRef.current === null
        ) {
            return;
        }

        streamControllerRef.current?.abort();
        setPendingUserMessage(null);
        setStreamingMessage(null);
        setStreaming(true);

        const activeSessionId = selectedSessionId;
        const assistantMessageId = activeTurn.assistant_message_id;
        const controller = api.streamTurn(activeSessionId, activeTurn.turn_id, {
            onEvent: (event) => {
                updateMessageInCache(activeSessionId, assistantMessageId, (message) => (
                    applyEventToAssistant(message, event)
                ));
            },
            onFinal: (chunk) => {
                applyFinalChunk(activeSessionId, chunk);
                setStreaming(false);
                streamControllerRef.current = null;
                streamingSessionIdRef.current = null;
                streamingTurnIdRef.current = null;
            },
            onError: (err) => {
                setStreaming(false);
                streamControllerRef.current = null;
                streamingSessionIdRef.current = null;
                streamingTurnIdRef.current = null;
                void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
                void queryClient.invalidateQueries({
                    queryKey: queryKeys.session(activeSessionId),
                });
                reportError(err);
            },
        });
        streamControllerRef.current = controller;
        streamingSessionIdRef.current = activeSessionId;
        streamingTurnIdRef.current = activeTurn.turn_id;
    }, [
        applyFinalChunk,
        queryClient,
        reportError,
        selectedSession?.active_turn,
        selectedSessionId,
        updateMessageInCache,
    ]);

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
        streamingTurnIdRef.current = null;

        const activeSessionId = selectedSessionId;
        const userMessage = createLocalMessage(activeSessionId, "user", message);
        const pendingAssistant = createLocalMessage(activeSessionId, "assistant", "");

        setPendingUserMessage(userMessage);
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
                applyFinalChunk(activeSessionId, chunk, userMessage);
                setPendingUserMessage(null);
                setStreamingMessage(null);
                setStreaming(false);
                streamControllerRef.current = null;
                streamingSessionIdRef.current = null;
                streamingTurnIdRef.current = null;
            },
            onError: (err) => {
                setStreamingMessage(null);
                setStreaming(false);
                streamControllerRef.current = null;
                streamingSessionIdRef.current = null;
                streamingTurnIdRef.current = null;
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

    function submitDraft() {
        if (!messageDraft.trim() || busy) return;
        const msg = messageDraft.trim();
        setMessageDraft("");
        sendMessage(msg);
    }

    function handleSendMessage(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        submitDraft();
    }

    function handleResetSession() {
        if (!selectedSessionId) return;
        streamControllerRef.current?.abort();
        streamControllerRef.current = null;
        streamingSessionIdRef.current = null;
        streamingTurnIdRef.current = null;
        setPendingUserMessage(null);
        setStreamingMessage(null);
        setStreaming(false);
        resetMutation.mutate(selectedSessionId);
    }

    function handleDeleteSession(sessionId: string) {
        if (
            !window.confirm(
                "Delete this session from history? This cannot be undone.",
            )
        ) {
            return;
        }
        if (selectedSessionId === sessionId || streamingSessionIdRef.current === sessionId) {
            streamControllerRef.current?.abort();
            streamControllerRef.current = null;
            streamingSessionIdRef.current = null;
            streamingTurnIdRef.current = null;
            setPendingUserMessage(null);
            setStreamingMessage(null);
            setStreaming(false);
        }
        deleteSessionMutation.mutate(sessionId);
    }

    const activeAssistantMessageId = selectedSession?.active_turn?.assistant_message_id ?? null;
    const busy = streaming || Boolean(selectedSession?.active_turn)
        || createSessionMutation.isPending || resetMutation.isPending
        || deleteSessionMutation.isPending;
    const transcriptMessages = selectedSession && pendingUserMessage
        && selectedSession.session_id === pendingUserMessage.session_id
        && !hasMessageWithId(selectedSession.messages, pendingUserMessage)
        && !hasEquivalentServerMessage(selectedSession.messages, pendingUserMessage)
        ? [...selectedSession.messages, pendingUserMessage]
        : (selectedSession?.messages ?? []);
    const selectedKernel = useMemo(() => {
        if (!selectedSession) {
            return null;
        }
        return kernels.find((kernel) => (
            kernel.session_id === selectedSession.agent_host_session_id
            || kernel.client_session_ids.includes(selectedSession.session_id)
        )) ?? null;
    }, [kernels, selectedSession]);
    const vscodeUrl = selectedKernel?.vscode_url
        ? browserReachableLocalUrl(selectedKernel.vscode_url)
        : null;
    const serviceUrl = selectedKernel?.free_port_url
        ? browserReachableLocalUrl(selectedKernel.free_port_url)
        : null;

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
                        <div
                            className={`session-row ${selectedSessionId === session.session_id ? "active" : ""}`}
                            key={session.session_id}
                        >
                            <button
                                className="session-item"
                                onClick={() => onSelectSession(session.session_id)}
                                type="button"
                            >
                                <strong>{session.agent_id}</strong>
                                <span className="muted">
                                    {session.message_count} messages · {session.status}
                                </span>
                            </button>
                            <button
                                aria-label={`Delete session ${session.session_id}`}
                                className="session-delete-button"
                                disabled={deleteSessionMutation.isPending}
                                onClick={() => handleDeleteSession(session.session_id)}
                                title="Delete session"
                                type="button"
                            >
                                Delete
                            </button>
                        </div>
                    ))}
                    {sessions.length === 0 && <div className="empty-state">No sessions yet</div>}
                </div>
            </aside>
            <section className="chat-main">
                {selectedSession ? (
                    <>
                        <div className="chat-header">
                            <div className="chat-header-title">
                                <h3>{selectedSession.agent_id}</h3>
                                <span className="muted">{selectedSession.session_id}</span>
                            </div>
                            <div className="chat-header-actions">
                                {selectedKernel ? (
                                    <>
                                        {vscodeUrl ? (
                                            <a
                                                className="secondary-button"
                                                href={vscodeUrl}
                                                target="_blank"
                                                rel="noreferrer"
                                            >
                                                Open VS Code
                                            </a>
                                        ) : (
                                            <button
                                                className="secondary-button"
                                                disabled
                                                title="VS Code unavailable"
                                                type="button"
                                            >
                                                Open VS Code
                                            </button>
                                        )}
                                        {serviceUrl ? (
                                            <a
                                                className="secondary-button"
                                                href={serviceUrl}
                                                target="_blank"
                                                rel="noreferrer"
                                            >
                                                Open service
                                            </a>
                                        ) : null}
                                    </>
                                ) : null}
                                <button
                                    className="secondary-button"
                                    disabled={busy}
                                    onClick={handleResetSession}
                                    type="button"
                                >
                                    Reset
                                </button>
                                <button
                                    className="danger-button"
                                    disabled={deleteSessionMutation.isPending}
                                    onClick={() => {
                                        if (selectedSessionId) {
                                            handleDeleteSession(selectedSessionId);
                                        }
                                    }}
                                    type="button"
                                >
                                    Delete
                                </button>
                            </div>
                        </div>
                        <div className="transcript">
                            {transcriptMessages.length > 0 || streamingMessage ? (
                                <>
                                    {transcriptMessages.map((msg) => {
                                        const messageStreaming = msg.message_id === activeAssistantMessageId;
                                        return (
                                            <article
                                                className={`message ${msg.role}${messageStreaming ? " streaming" : ""}`}
                                                key={msg.message_id}
                                            >
                                                <header>{msg.role}</header>
                                                {msg.reasoning && (
                                                    <details className="reasoning-block">
                                                        <summary>Reasoning</summary>
                                                        <div className="reasoning-content">{msg.reasoning}</div>
                                                    </details>
                                                )}
                                                <MessageMarkdown
                                                    content={msg.content}
                                                    toolCalls={msg.tool_calls}
                                                    onSelectToolCall={setSelectedToolCall}
                                                    streaming={messageStreaming}
                                                />
                                            </article>
                                        );
                                    })}
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
                                            <MessageMarkdown
                                                content={streamingMessage.content}
                                                toolCalls={streamingMessage.tool_calls}
                                                onSelectToolCall={setSelectedToolCall}
                                                streaming
                                            />
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
                                        submitDraft();
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
