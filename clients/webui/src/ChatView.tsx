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
    Add20Regular,
    ArrowClockwise20Regular,
    Chat24Regular,
    Code20Regular,
    Delete20Regular,
    MoreHorizontal20Regular,
    Open20Regular,
    Save20Regular,
    Send20Regular,
} from "@fluentui/react-icons";
import {
    queryKeys,
    useAgents,
    useKernels,
    useSession,
    useSessions,
} from "./queries";
import { useErrorContext } from "./useErrorContext";
import { promptSaveWorkspace, promptWorkspaceSaveDetails } from "./saveWorkspacePrompt";
import {
    Button,
    Field,
    Input,
    Menu,
    MenuItem,
    MenuList,
    MenuPopover,
    MenuTrigger,
    Select,
    Textarea,
    Tooltip,
} from "./fluent";
import { EmptyState, FormDialog, StatusBadge } from "./ui";
import { sessionTone } from "./status";
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
                                    <Button
                                        appearance="outline"
                                        className="inline-tool-call"
                                        onClick={() => onSelectToolCall?.(toolCall)}
                                        size="small"
                                        type="button"
                                    >
                                        {children}
                                    </Button>
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
    const [selectedNewSessionAgentId, setNewSessionAgentId] = useState("");
    const [newSessionChannelName, setNewSessionChannelName] = useState("");
    const [showNewSession, setShowNewSession] = useState(false);
    const [selectedToolCall, setSelectedToolCall] = useState<ToolCall | null>(null);

    // Fall back to the first agent until the user picks one explicitly.
    const newSessionAgentId = selectedNewSessionAgentId || (agents[0]?.agent_id ?? "");

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
    const deleteAllSessionsMutation = useMutation({
        mutationFn: async (sessionIds: string[]) => {
            for (const sessionId of sessionIds) {
                await api.deleteSession(sessionId);
            }
        },
        onSuccess: (_, sessionIds) => {
            for (const sessionId of sessionIds) {
                queryClient.removeQueries({ queryKey: queryKeys.session(sessionId) });
            }
            if (selectedSessionId !== null && sessionIds.includes(selectedSessionId)) {
                onSelectSession(null);
            }
        },
        onError: reportError,
        onSettled: () => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            void queryClient.invalidateQueries({ queryKey: queryKeys.kernels });
        },
    });
    const saveWorkspaceMutation = useMutation({
        mutationFn: ({ sessionId, workspace_id, name }: { sessionId: string; workspace_id: string; name: string }) =>
            api.saveSessionWorkspace(sessionId, { workspace_id, name }),
        onSuccess: () => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.workspaces });
        },
        onError: reportError,
    });

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

    async function handleCreateSession() {
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

    function handleDeleteAllSessions() {
        const sessionIds = sessions.map((session) => session.session_id);
        if (sessionIds.length === 0) {
            return;
        }
        const targetLabel = sessionIds.length === 1
            ? "the only workspace session"
            : `all ${sessionIds.length} workspace sessions`;
        const confirmed = window.confirm(
            `Delete ${targetLabel}? This will destroy unsaved workspaces forever and cannot be undone.`,
        );
        if (!confirmed) {
            return;
        }
        streamControllerRef.current?.abort();
        streamControllerRef.current = null;
        streamingSessionIdRef.current = null;
        streamingTurnIdRef.current = null;
        setPendingUserMessage(null);
        setStreamingMessage(null);
        setStreaming(false);
        deleteAllSessionsMutation.mutate(sessionIds);
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
    const deletingSessions = deleteSessionMutation.isPending || deleteAllSessionsMutation.isPending;
    const busy = effectiveStreaming || Boolean(selectedSession?.active_turn)
        || createSessionMutation.isPending || resetMutation.isPending
        || deletingSessions || saveWorkspaceMutation.isPending;
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
    const selectedTone = sessionTone(selectedSession?.status ?? "");

    return (
        <div className="chat-layout">
            <aside className="session-rail">
                <div className="session-rail-header">
                    <h2>Sessions</h2>
                    <div className="session-rail-header-actions">
                        <Tooltip content="New session" relationship="label">
                            <Button
                                appearance="subtle"
                                icon={<Add20Regular />}
                                onClick={() => setShowNewSession(true)}
                                size="small"
                            />
                        </Tooltip>
                        <Menu positioning="below-end">
                            <MenuTrigger disableButtonEnhancement>
                                <Tooltip content="Session actions" relationship="label">
                                    <Button
                                        appearance="subtle"
                                        icon={<MoreHorizontal20Regular />}
                                        size="small"
                                    />
                                </Tooltip>
                            </MenuTrigger>
                            <MenuPopover>
                                <MenuList>
                                    <MenuItem
                                        disabled={sessions.length === 0 || deletingSessions
                                            || saveWorkspaceMutation.isPending}
                                        icon={<Delete20Regular />}
                                        onClick={handleDeleteAllSessions}
                                        style={{ color: "var(--danger)" }}
                                    >
                                        Delete all sessions
                                    </MenuItem>
                                </MenuList>
                            </MenuPopover>
                        </Menu>
                    </div>
                </div>
                <div aria-label="Sessions" className="session-list">
                    {sessions.map((session) => (
                        <div
                            className={`session-row${
                                selectedSessionId === session.session_id ? " active" : ""
                            }`}
                            key={session.session_id}
                        >
                            <button
                                aria-current={selectedSessionId === session.session_id}
                                className="session-row-button"
                                onClick={() => onSelectSession(session.session_id)}
                                title={session.session_id}
                                type="button"
                            >
                                <span className="session-row-title">
                                    <span
                                        aria-hidden="true"
                                        className={`status-dot ${sessionTone(session.status)}`}
                                    />
                                    <span className="truncate">{session.agent_id}</span>
                                </span>
                                <span className="session-row-meta">
                                    <span className="truncate">
                                        {sessionChannelLabel(session.channel_name)}
                                    </span>
                                    <span aria-hidden="true">·</span>
                                    <span className="nowrap">{session.message_count} msg</span>
                                </span>
                            </button>
                            <Tooltip content="Delete session" relationship="label">
                                <Button
                                    appearance="subtle"
                                    className="session-row-delete"
                                    disabled={deletingSessions || saveWorkspaceMutation.isPending}
                                    icon={<Delete20Regular />}
                                    onClick={() => void handleDeleteSession(session.session_id)}
                                    size="small"
                                />
                            </Tooltip>
                        </div>
                    ))}
                    {sessions.length === 0 && (
                        <p className="session-list-empty">
                            No sessions yet. Start one to talk to an agent.
                        </p>
                    )}
                </div>
            </aside>

            <section className="chat-main">
                {selectedSession
                    ? (
                        <>
                            <header className="chat-header">
                                <div className="chat-header-title">
                                    <div className="chat-header-heading">
                                        <h2>{selectedSession.agent_id}</h2>
                                        <StatusBadge
                                            label={selectedSession.status}
                                            tone={selectedTone}
                                        />
                                    </div>
                                    <div className="chat-header-meta">
                                        <span title={selectedSession.session_id}>
                                            {compactSessionId(selectedSession.session_id)}
                                        </span>
                                        <span aria-hidden="true">·</span>
                                        <span>
                                            {sessionChannelLabel(selectedSession.channel_name)}
                                        </span>
                                        <span aria-hidden="true">·</span>
                                        <span>{transcriptMessages.length} messages</span>
                                        <span aria-hidden="true">·</span>
                                        <span>
                                            kernel {selectedKernel?.status ?? "not attached"}
                                        </span>
                                    </div>
                                </div>
                                <div className="chat-header-actions">
                                    {vscodeUrl !== null && (
                                        <Button
                                            as="a"
                                            href={vscodeUrl}
                                            icon={<Code20Regular />}
                                            rel="noreferrer"
                                            size="small"
                                            target="_blank"
                                        >
                                            VS Code
                                        </Button>
                                    )}
                                    <Menu positioning="below-end">
                                        <MenuTrigger disableButtonEnhancement>
                                            <Tooltip content="Session actions" relationship="label">
                                                <Button
                                                    appearance="subtle"
                                                    icon={<MoreHorizontal20Regular />}
                                                    size="small"
                                                />
                                            </Tooltip>
                                        </MenuTrigger>
                                        <MenuPopover>
                                            <MenuList>
                                                {serviceUrl !== null && (
                                                    <MenuItem
                                                        icon={<Open20Regular />}
                                                        onClick={() =>
                                                            window.open(
                                                                serviceUrl,
                                                                "_blank",
                                                                "noreferrer",
                                                            )}
                                                    >
                                                        Open forwarded service
                                                    </MenuItem>
                                                )}
                                                {selectedKernel !== null && (
                                                    <MenuItem
                                                        disabled={busy}
                                                        icon={<Save20Regular />}
                                                        onClick={() =>
                                                            void handleSaveWorkspace(
                                                                selectedSession.session_id,
                                                            )}
                                                    >
                                                        Save workspace
                                                    </MenuItem>
                                                )}
                                                <MenuItem
                                                    disabled={busy}
                                                    icon={<ArrowClockwise20Regular />}
                                                    onClick={handleResetSession}
                                                >
                                                    Reset transcript
                                                </MenuItem>
                                                <MenuItem
                                                    disabled={deletingSessions
                                                        || saveWorkspaceMutation.isPending}
                                                    icon={<Delete20Regular />}
                                                    onClick={() => {
                                                        if (selectedSessionId !== null) {
                                                            void handleDeleteSession(
                                                                selectedSessionId,
                                                            );
                                                        }
                                                    }}
                                                    style={{ color: "var(--danger)" }}
                                                >
                                                    Delete session
                                                </MenuItem>
                                            </MenuList>
                                        </MenuPopover>
                                    </Menu>
                                </div>
                            </header>

                            <div
                                aria-live={effectiveStreaming ? "polite" : "off"}
                                className="transcript"
                            >
                                {transcriptMessages.length > 0 || displayedStreamingMessage
                                    ? (
                                        <div className="transcript-inner">
                                            {transcriptMessages.map((msg) => {
                                                const messageStreaming = effectiveStreaming
                                                    && msg.message_id === activeAssistantMessageId;
                                                return (
                                                    <article
                                                        className={`message ${msg.role}`}
                                                        key={msg.message_id}
                                                    >
                                                        <header className="message-header">
                                                            <span className="message-role">
                                                                {messageRoleLabel(msg.role)}
                                                            </span>
                                                            <time dateTime={msg.created_at}>
                                                                {formatTimestamp(msg.created_at)}
                                                            </time>
                                                        </header>
                                                        {msg.reasoning && (
                                                            <details className="reasoning-block">
                                                                <summary>Reasoning</summary>
                                                                <div className="reasoning-content">
                                                                    {msg.reasoning}
                                                                </div>
                                                            </details>
                                                        )}
                                                        <MessageMarkdown
                                                            content={msg.content}
                                                            onSelectToolCall={setSelectedToolCall}
                                                            streaming={messageStreaming}
                                                            toolCalls={msg.tool_calls}
                                                        />
                                                    </article>
                                                );
                                            })}
                                            {displayedStreamingMessage && (
                                                <article
                                                    className={`message ${displayedStreamingMessage.role}`}
                                                    key={displayedStreamingMessage.message_id}
                                                >
                                                    <header className="message-header">
                                                        <span className="message-role">
                                                            {messageRoleLabel(
                                                                displayedStreamingMessage.role,
                                                            )}
                                                        </span>
                                                        <time
                                                            dateTime={displayedStreamingMessage
                                                                .created_at}
                                                        >
                                                            {formatTimestamp(
                                                                displayedStreamingMessage
                                                                    .created_at,
                                                            )}
                                                        </time>
                                                    </header>
                                                    {displayedStreamingMessage.reasoning && (
                                                        <details className="reasoning-block" open>
                                                            <summary>Reasoning</summary>
                                                            <div className="reasoning-content">
                                                                {displayedStreamingMessage
                                                                    .reasoning}
                                                            </div>
                                                        </details>
                                                    )}
                                                    <MessageMarkdown
                                                        content={displayedStreamingMessage.content}
                                                        onSelectToolCall={setSelectedToolCall}
                                                        streaming
                                                        toolCalls={displayedStreamingMessage
                                                            .tool_calls}
                                                    />
                                                </article>
                                            )}
                                        </div>
                                    )
                                    : (
                                        <div className="transcript-inner">
                                            <EmptyState
                                                description="Send the first prompt. Reasoning, tool calls, and streaming output all appear inline."
                                                icon={<Chat24Regular />}
                                                title="No messages yet"
                                            />
                                        </div>
                                    )}
                            </div>

                            <form className="composer" onSubmit={handleSendMessage}>
                                <div className="composer-shell">
                                    <Textarea
                                        appearance="filled-lighter"
                                        onChange={(e) => setMessageDraft(e.target.value)}
                                        onKeyDown={(e: KeyboardEvent<HTMLTextAreaElement>) => {
                                            if (e.key === "Enter" && !e.shiftKey) {
                                                e.preventDefault();
                                                submitDraft();
                                            }
                                        }}
                                        placeholder="Ask the agent to inspect, edit, run, or explain…"
                                        rows={3}
                                        value={messageDraft}
                                    />
                                    <div className="composer-footer">
                                        <span className="muted-sm">
                                            Enter to send, Shift+Enter for a new line
                                        </span>
                                        <Button
                                            appearance="primary"
                                            disabled={busy || !messageDraft.trim()}
                                            icon={<Send20Regular />}
                                            type="submit"
                                        >
                                            Send
                                        </Button>
                                    </div>
                                </div>
                            </form>
                        </>
                    )
                    : (
                        <div className="chat-placeholder">
                            <EmptyState
                                action={
                                    <Button
                                        appearance="primary"
                                        icon={<Add20Regular />}
                                        onClick={() => setShowNewSession(true)}
                                    >
                                        New session
                                    </Button>
                                }
                                description="Pick a session from the list, or start a new one to talk to an agent."
                                icon={<Chat24Regular />}
                                title="No session selected"
                            />
                        </div>
                    )}
            </section>

            <FormDialog
                busy={busy || !newSessionAgentId}
                onOpenChange={setShowNewSession}
                onSubmit={() => {
                    void handleCreateSession();
                }}
                open={showNewSession}
                submitLabel="Start session"
                title="New session"
            >
                <Field label="Agent" required>
                    <Select
                        onChange={(e) => setNewSessionAgentId(e.target.value)}
                        value={newSessionAgentId}
                    >
                        {agents.map((a) => (
                            <option key={a.agent_id} value={a.agent_id}>{a.name}</option>
                        ))}
                    </Select>
                </Field>
                <Field
                    hint="Optional. Groups related sessions under a shared name."
                    label="Channel"
                >
                    <Input
                        onChange={(e) => setNewSessionChannelName(e.target.value)}
                        placeholder="default channel"
                        value={newSessionChannelName}
                    />
                </Field>
            </FormDialog>

            <ToolDetailPane
                onClose={() => setSelectedToolCall(null)}
                toolCall={selectedToolCall}
            />
        </div>
    );
}
