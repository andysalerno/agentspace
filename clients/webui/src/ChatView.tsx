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
import { promptSaveWorkspace, promptWorkspaceSaveDetails } from "./saveWorkspacePrompt";
import "./chat-workspace.css";

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

function compactSessionId(sessionId: string): string {
    if (sessionId.length <= 18) {
        return sessionId;
    }
    return `${sessionId.slice(0, 8)}…${sessionId.slice(-6)}`;
}

function formatTimestamp(value: string): string {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) {
        return value;
    }
    return new Intl.DateTimeFormat(undefined, {
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        month: "short",
    }).format(date);
}

function messageRoleLabel(role: string): string {
    if (role === "user") {
        return "You";
    }
    if (role === "assistant") {
        return "Agent";
    }
    return role;
}

function sessionChannelLabel(channelName: string | null): string {
    return channelName?.trim() || "default channel";
}

function statusTone(status: string | undefined): "active" | "busy" | "error" | "neutral" {
    const normalized = status?.toLowerCase() ?? "";
    if (/(error|fail|stopped|unhealthy|cancel)/.test(normalized)) {
        return "error";
    }
    if (/(active|busy|pending|running|start|stream|working)/.test(normalized)) {
        return "busy";
    }
    if (/(ready|idle|complete|success|ok|online)/.test(normalized)) {
        return "active";
    }
    return "neutral";
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

function serializedToolCalls(message: ChatMessage): string {
    return JSON.stringify(message.tool_calls ?? []);
}

function mergeAssistantProgress(local: ChatMessage, persisted: ChatMessage): ChatMessage {
    const localReasoning = local.reasoning ?? "";
    const persistedReasoning = persisted.reasoning ?? "";
    const localToolCalls = serializedToolCalls(local);
    const persistedToolCalls = serializedToolCalls(persisted);
    const next: ChatMessage = {
        ...local,
        content: persisted.content.length > local.content.length
            ? persisted.content
            : local.content,
        reasoning: persistedReasoning.length > localReasoning.length
            ? persisted.reasoning
            : local.reasoning,
        tool_calls: persistedToolCalls.length > localToolCalls.length
            || (persistedToolCalls !== localToolCalls && persistedToolCalls.length === localToolCalls.length)
            ? persisted.tool_calls
            : local.tool_calls,
    };

    if (
        next.content === local.content
        && next.reasoning === local.reasoning
        && serializedToolCalls(next) === localToolCalls
    ) {
        return local;
    }

    return next;
}

function findAssistantAfterUser(
    messages: ChatMessage[],
    userMessage: ChatMessage,
): ChatMessage | null {
    const userIndex = messages.findIndex(
        (message) =>
            message.message_id === userMessage.message_id
            || (
                message.session_id === userMessage.session_id
                && message.role === userMessage.role
                && message.content === userMessage.content
            ),
    );
    if (userIndex < 0) {
        return null;
    }
    return messages.slice(userIndex + 1).find((message) => message.role === "assistant") ?? null;
}

function isRecoverableStreamError(error: Error): boolean {
    return error.message === "message stream ended without a final payload";
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
    const { data: selectedSession = null } = useSession(selectedSessionId);
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
    const saveWorkspaceMutation = useMutation({
        mutationFn: ({ sessionId, workspace_id, name }: { sessionId: string; workspace_id: string; name: string }) =>
            api.saveSessionWorkspace(sessionId, { workspace_id, name }),
        onSuccess: () => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.workspaces });
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
                if (!isRecoverableStreamError(err)) {
                    reportError(err);
                }
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
                if (!isRecoverableStreamError(err)) {
                    reportError(err);
                }
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

    async function handleDeleteSession(sessionId: string) {
        const decision = promptSaveWorkspace();
        if (decision.action === "cancel") {
            return;
        }
        if (decision.action === "save") {
            try {
                await saveWorkspaceMutation.mutateAsync({ sessionId, ...decision });
            } catch {
                return;
            }
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

    async function handleSaveWorkspace(sessionId: string) {
        const details = promptWorkspaceSaveDetails();
        if (details === null) {
            return;
        }
        try {
            await saveWorkspaceMutation.mutateAsync({ sessionId, ...details });
            window.alert(`Workspace "${details.name}" saved.`);
        } catch {
            return;
        }
    }

    const cachedMessages = selectedSession?.messages ?? [];
    const activeAssistantMessageId = selectedSession?.active_turn?.assistant_message_id ?? null;
    const completedAssistantFromCache = selectedSession && pendingUserMessage
        && selectedSession.session_id === pendingUserMessage.session_id
        && !selectedSession.active_turn
        ? findAssistantAfterUser(cachedMessages, pendingUserMessage)
        : null;
    const streamCompletedFromCache = Boolean(streamingMessage && completedAssistantFromCache);
    const persistedStreamingAssistant = streamingMessage && activeAssistantMessageId
        ? cachedMessages.find((message) => message.message_id === activeAssistantMessageId) ?? null
        : null;
    const displayedStreamingMessage = streamCompletedFromCache
        ? null
        : (streamingMessage && persistedStreamingAssistant
            ? mergeAssistantProgress(streamingMessage, persistedStreamingAssistant)
            : streamingMessage);
    const effectiveStreaming = streaming && !streamCompletedFromCache;
    const busy = effectiveStreaming || Boolean(selectedSession?.active_turn)
        || createSessionMutation.isPending || resetMutation.isPending
        || deleteSessionMutation.isPending || saveWorkspaceMutation.isPending;
    const visibleCachedMessages = displayedStreamingMessage && activeAssistantMessageId
        ? cachedMessages.filter((message) => message.message_id !== activeAssistantMessageId)
        : cachedMessages;
    const transcriptMessages = selectedSession && pendingUserMessage
        && selectedSession.session_id === pendingUserMessage.session_id
        && !hasMessageWithId(cachedMessages, pendingUserMessage)
        && !hasEquivalentServerMessage(cachedMessages, pendingUserMessage)
        ? [...visibleCachedMessages, pendingUserMessage]
        : visibleCachedMessages;

    useEffect(() => {
        if (!streamCompletedFromCache) {
            return;
        }
        streamControllerRef.current?.abort();
        streamControllerRef.current = null;
        streamingSessionIdRef.current = null;
        streamingTurnIdRef.current = null;
    }, [streamCompletedFromCache]);

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
    const selectedStatusTone = statusTone(selectedSession?.status);

    return (
        <div className="chat-layout">
            <aside className="chat-sessions-panel chat-session-rail">
                <div className="chat-sessions-heading">
                    <div className="rail-title-stack">
                        <span className="rail-eyebrow">Workspace</span>
                        <h3>
                            Sessions
                            <span className="rail-count">{sessions.length}</span>
                        </h3>
                    </div>
                    <button
                        className="icon-button new-session-button"
                        onClick={() => setShowNewSession(!showNewSession)}
                        type="button"
                        title="New session"
                        aria-expanded={showNewSession}
                    >
                        {showNewSession ? "×" : "+"}
                    </button>
                </div>
                {showNewSession && (
                    <form className="compact-form new-session-form" onSubmit={(e) => { void handleCreateSession(e); }}>
                        <label>
                            <span>Agent</span>
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
                        </label>
                        <label>
                            <span>Channel</span>
                            <input
                                placeholder="default channel"
                                value={newSessionChannelName}
                                onChange={(e) => setNewSessionChannelName(e.target.value)}
                            />
                        </label>
                        <button disabled={busy || !newSessionAgentId} type="submit">
                            Start session
                        </button>
                    </form>
                )}
                <div className="session-list" aria-label="Sessions">
                    {sessions.map((session) => {
                        const tone = statusTone(session.status);
                        return (
                            <div
                                className={`session-row chat-session-row ${selectedSessionId === session.session_id ? "active" : ""}`}
                                key={session.session_id}
                            >
                                <button
                                    className="session-item chat-session-card"
                                    onClick={() => onSelectSession(session.session_id)}
                                    type="button"
                                    title={session.session_id}
                                >
                                    <span className={`status-dot status-${tone}`} aria-hidden="true" />
                                    <span className="session-card-main">
                                        <strong title={session.agent_id}>{session.agent_id}</strong>
                                        <span className="session-card-meta">
                                            <span title={session.session_id}>{compactSessionId(session.session_id)}</span>
                                            <span>{session.message_count} msg</span>
                                        </span>
                                        <span className="session-card-channel" title={sessionChannelLabel(session.channel_name)}>
                                            {sessionChannelLabel(session.channel_name)}
                                        </span>
                                    </span>
                                    <span className={`session-status-pill status-${tone}`}>
                                        {session.status}
                                    </span>
                                </button>
                                <button
                                    aria-label={`Delete session ${session.session_id}`}
                                    className="session-delete-button"
                                    disabled={deleteSessionMutation.isPending || saveWorkspaceMutation.isPending}
                                    onClick={() => void handleDeleteSession(session.session_id)}
                                    title="Delete session"
                                    type="button"
                                >
                                    ×
                                </button>
                            </div>
                        );
                    })}
                    {sessions.length === 0 && (
                        <div className="empty-state rail-empty-state">
                            <span>No sessions yet</span>
                            <button className="secondary-button small" onClick={() => setShowNewSession(true)} type="button">
                                Create one
                            </button>
                        </div>
                    )}
                </div>
            </aside>
            <section className="chat-main">
                {selectedSession ? (
                    <>
                        <div className="chat-header chat-workspace-header">
                            <div className="chat-header-title">
                                <div className="workspace-title-row">
                                    <span className={`status-dot status-${selectedStatusTone}`} aria-hidden="true" />
                                    <h2>{selectedSession.agent_id}</h2>
                                    <span className={`workspace-status-chip status-${selectedStatusTone}`}>
                                        {selectedSession.status}
                                    </span>
                                </div>
                                <div className="workspace-meta-grid">
                                    <span>
                                        <span>session</span>
                                        <code title={selectedSession.session_id}>{selectedSession.session_id}</code>
                                    </span>
                                    <span>
                                        <span>channel</span>
                                        <strong>{sessionChannelLabel(selectedSession.channel_name)}</strong>
                                    </span>
                                    <span>
                                        <span>messages</span>
                                        <strong>{transcriptMessages.length}</strong>
                                    </span>
                                    <span>
                                        <span>kernel</span>
                                        <strong>{selectedKernel?.status ?? "not attached"}</strong>
                                    </span>
                                </div>
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
                                                VS Code
                                            </a>
                                        ) : (
                                            <button
                                                className="secondary-button"
                                                disabled
                                                title="VS Code unavailable"
                                                type="button"
                                            >
                                                VS Code
                                            </button>
                                        )}
                                        {serviceUrl ? (
                                            <a
                                                className="secondary-button"
                                                href={serviceUrl}
                                                target="_blank"
                                                rel="noreferrer"
                                            >
                                                Service
                                            </a>
                                        ) : null}
                                        <button
                                            className="secondary-button"
                                            disabled={busy}
                                            onClick={() => void handleSaveWorkspace(selectedSession.session_id)}
                                            type="button"
                                        >
                                            Save workspace
                                        </button>
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
                                    disabled={deleteSessionMutation.isPending || saveWorkspaceMutation.isPending}
                                    onClick={() => {
                                        if (selectedSessionId) {
                                            void handleDeleteSession(selectedSessionId);
                                        }
                                    }}
                                    type="button"
                                >
                                    Delete
                                </button>
                            </div>
                        </div>
                        <div className="transcript chat-transcript" aria-live={effectiveStreaming ? "polite" : "off"}>
                            {transcriptMessages.length > 0 || displayedStreamingMessage ? (
                                <>
                                    {transcriptMessages.map((msg) => {
                                        const messageStreaming = effectiveStreaming && msg.message_id === activeAssistantMessageId;
                                        return (
                                            <article
                                                className={`message chat-message ${msg.role}${messageStreaming ? " streaming" : ""}`}
                                                key={msg.message_id}
                                            >
                                                <header className="message-header">
                                                    <span className="message-role">{messageRoleLabel(msg.role)}</span>
                                                    <time dateTime={msg.created_at}>{formatTimestamp(msg.created_at)}</time>
                                                </header>
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
                                    {displayedStreamingMessage && (
                                        <article
                                            className={`message chat-message ${displayedStreamingMessage.role} streaming`}
                                            key={displayedStreamingMessage.message_id}
                                        >
                                            <header className="message-header">
                                                <span className="message-role">{messageRoleLabel(displayedStreamingMessage.role)}</span>
                                                <time dateTime={displayedStreamingMessage.created_at}>{formatTimestamp(displayedStreamingMessage.created_at)}</time>
                                            </header>
                                            {displayedStreamingMessage.reasoning && (
                                                <details className="reasoning-block" open>
                                                    <summary>Reasoning</summary>
                                                    <div className="reasoning-content">
                                                        {displayedStreamingMessage.reasoning}
                                                    </div>
                                                </details>
                                            )}
                                            <MessageMarkdown
                                                content={displayedStreamingMessage.content}
                                                toolCalls={displayedStreamingMessage.tool_calls}
                                                onSelectToolCall={setSelectedToolCall}
                                                streaming
                                            />
                                        </article>
                                    )}
                                </>
                            ) : (
                                <div className="empty-state centered chat-empty-state">
                                    <div className="empty-state-kicker">No transcript</div>
                                    <h3>Ready for a focused agent turn</h3>
                                    <p>Send the first prompt. Tool calls, reasoning, and streaming output stay inline.</p>
                                </div>
                            )}
                        </div>
                        <form className="composer chat-composer" onSubmit={handleSendMessage}>
                            <div className="composer-input-shell">
                                <div className="composer-toolbar">
                                    <span>{sessionChannelLabel(selectedSession.channel_name)}</span>
                                    <span>Enter sends · Shift+Enter newline</span>
                                </div>
                                <textarea
                                    placeholder="Ask the agent to inspect, edit, run, or explain…"
                                    rows={2}
                                    value={messageDraft}
                                    onChange={(e) => setMessageDraft(e.target.value)}
                                    onKeyDown={(e: KeyboardEvent<HTMLTextAreaElement>) => {
                                        if (e.key === "Enter" && !e.shiftKey) {
                                            e.preventDefault();
                                            submitDraft();
                                        }
                                    }}
                                />
                            </div>
                            <button className="composer-send-button" disabled={busy || !messageDraft.trim()} type="submit">
                                Send
                            </button>
                        </form>
                    </>
                ) : (
                    <div className="empty-state centered full-height chat-empty-state chat-empty-workspace">
                        <div className="empty-state-kicker">AgentSpace chat</div>
                        <h3>Select a session to enter the workspace</h3>
                        <p>Create a fresh channel or jump back into an existing session from the rail.</p>
                        <button className="secondary-button" onClick={() => setShowNewSession(true)} type="button">
                            New session
                        </button>
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
