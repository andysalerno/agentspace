import { useCallback, useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
    Add20Regular,
    ArrowClockwise20Regular,
    Code20Regular,
    Delete20Regular,
    History20Regular,
    MoreHorizontal20Regular,
    Save20Regular,
    Stop20Regular,
    WindowConsole20Regular,
} from "@fluentui/react-icons";
import { api } from "./api";
import { browserReachableLocalUrl } from "./browserUrls";
import {
    queryKeys,
    useAgents,
    useKernels,
    useSession,
    useSessions,
    useTerminalStatus,
} from "./queries";
import { promptSaveWorkspace, promptWorkspaceSaveDetails } from "./saveWorkspacePrompt";
import type {
    SessionSummary,
    TerminalAttachKind,
    TerminalStatus,
} from "./types";
import {
    Button,
    Field,
    Menu,
    MenuItem,
    MenuList,
    MenuPopover,
    MenuTrigger,
    Select,
    Tooltip,
} from "./fluent";
import { EmptyState, FormDialog, LoadingState, StatusBadge } from "./ui";
import { statusTone } from "./status";
import Terminal from "./Terminal";
import type {
    TerminalAttachment,
    TerminalConnectionState,
} from "./Terminal";
import "./chat-workspace.css";
import "./cli-view.css";

type CliViewProps = {
    selectedSessionId: string | null;
    onSelectSession: (sessionId: string | null) => void;
    darkMode: boolean;
};

type OperationState = "starting" | "resuming" | "stopping" | null;

function compactId(value: string): string {
    if (value.length <= 18) {
        return value;
    }
    return `${value.slice(0, 8)}…${value.slice(-6)}`;
}

function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
}

function runtimeLabel(
    session: SessionSummary,
    terminal: TerminalStatus | null,
    connectionState: TerminalConnectionState,
    operation: OperationState,
): string {
    if (session.recovery_state === "legacy-unrecoverable") {
        return "legacy-unrecoverable";
    }
    if (operation !== null) {
        return operation;
    }
    if (connectionState === "ready") {
        return "live";
    }
    if (connectionState === "reconnecting" || connectionState === "disconnected") {
        return "disconnected";
    }
    if (connectionState === "error") {
        return "error";
    }
    if (terminal?.state === "exited") {
        return "exited";
    }
    if (terminal?.state === "missing") {
        return "disconnected";
    }
    return session.runtime_status ?? session.status;
}

function attachKindLabel(kind: TerminalAttachKind | null): string | null {
    switch (kind) {
        case "started":
            return "Started";
        case "attached":
            return "Attached";
        case "resumed":
            return "Resumed";
        default:
            return null;
    }
}

export default function CliView({
    selectedSessionId,
    onSelectSession,
    darkMode,
}: CliViewProps) {
    const queryClient = useQueryClient();
    const { data: agents = [] } = useAgents();
    const { data: allSessions = [] } = useSessions();
    const sessions = allSessions.filter((session) => session.interaction_mode === "cli");
    const { data: kernels = [] } = useKernels();
    const selectedSessionQuery = useSession(selectedSessionId);
    const selectedSession = selectedSessionQuery.data;
    const cliAgents = agents.filter((agent) => agent.cli !== null);
    const agentMap = useMemo(
        () => new Map(agents.map((agent) => [agent.agent_id, agent])),
        [agents],
    );

    const [showNewSession, setShowNewSession] = useState(false);
    const [newSessionAgentId, setNewSessionAgentId] = useState("");
    const [creating, setCreating] = useState(false);
    const [operation, setOperation] = useState<OperationState>(null);
    const [operationError, setOperationError] = useState<string | null>(null);
    const [localTerminalStatus, setLocalTerminalStatus] = useState<TerminalStatus | null>(null);
    const [connectionState, setConnectionState] =
        useState<TerminalConnectionState>("disconnected");
    const [attachment, setAttachment] = useState<TerminalAttachment | null>(null);
    const [attachKind, setAttachKind] = useState<TerminalAttachKind | null>(null);
    const [copyMode, setCopyMode] = useState(false);
    const [reconnectKey, setReconnectKey] = useState(0);
    const [saving, setSaving] = useState(false);
    const [deleting, setDeleting] = useState(false);

    const terminalQueryEnabled = selectedSessionId !== null
        && localTerminalStatus !== null
        && operation !== "starting"
        && selectedSession?.recovery_state !== "legacy-unrecoverable"
        && selectedSession?.runtime_status !== "error";
    const terminalStatusQuery = useTerminalStatus(
        selectedSessionId,
        terminalQueryEnabled,
    );
    const terminalStatus = terminalStatusQuery.data ?? localTerminalStatus;

    const cacheTerminalStatus = useCallback(
        (sessionId: string, status: TerminalStatus) => {
            setLocalTerminalStatus(status);
            queryClient.setQueryData(queryKeys.terminal(sessionId), status);
            if (status.attach_kind !== null) {
                setAttachKind(status.attach_kind);
            }
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            void queryClient.invalidateQueries({ queryKey: queryKeys.session(sessionId) });
        },
        [queryClient],
    );

    useEffect(() => {
        let cancelled = false;
        void Promise.resolve().then(async () => {
            if (cancelled || selectedSessionId === null) {
                return;
            }
            setOperation("starting");
            try {
                const status = await api.ensureTerminal(selectedSessionId);
                if (cancelled) {
                    return;
                }
                cacheTerminalStatus(selectedSessionId, status);
                setOperation(null);
            } catch (error) {
                if (cancelled) {
                    return;
                }
                setOperationError(errorMessage(error));
                setOperation(null);
                setConnectionState("error");
            }
        });
        return () => {
            cancelled = true;
        };
    }, [cacheTerminalStatus, selectedSessionId]);

    function selectSession(sessionId: string | null) {
        setOperationError(null);
        setLocalTerminalStatus(null);
        setConnectionState("disconnected");
        setAttachment(null);
        setAttachKind(null);
        setCopyMode(false);
        setOperation(sessionId === null ? null : "starting");
        onSelectSession(sessionId);
    }

    const selectedKernel = useMemo(() => {
        if (selectedSession === undefined) {
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

    const handleLifecycleStatus = useCallback((status: TerminalStatus) => {
        if (selectedSessionId !== null) {
            cacheTerminalStatus(selectedSessionId, status);
        }
    }, [cacheTerminalStatus, selectedSessionId]);

    async function handleCreateSession() {
        const agentId = newSessionAgentId || cliAgents[0]?.agent_id;
        if (agentId === undefined) {
            return;
        }
        setCreating(true);
        setOperationError(null);
        try {
            const session = await api.createSession({
                agent_id: agentId,
                channel_name: null,
                client_type: "webui",
                interaction_mode: "cli",
            });
            await queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            selectSession(session.session_id);
            setShowNewSession(false);
            setNewSessionAgentId("");
        } catch (error) {
            setOperationError(errorMessage(error));
        } finally {
            setCreating(false);
        }
    }

    async function handleReconnect() {
        if (selectedSessionId === null) {
            return;
        }
        setOperationError(null);
        setCopyMode(false);
        if (terminalStatus?.state === "exited") {
            setOperation("resuming");
            try {
                const status = await api.resumeTerminal(selectedSessionId);
                cacheTerminalStatus(selectedSessionId, status);
                setReconnectKey((value) => value + 1);
            } catch (error) {
                setOperationError(errorMessage(error));
                setConnectionState("error");
            } finally {
                setOperation(null);
            }
            return;
        }

        setOperation("starting");
        try {
            const status = await api.ensureTerminal(selectedSessionId);
            cacheTerminalStatus(selectedSessionId, status);
            setReconnectKey((value) => value + 1);
        } catch (error) {
            setOperationError(errorMessage(error));
            setConnectionState("error");
        } finally {
            setOperation(null);
        }
    }

    async function handleStop() {
        if (selectedSessionId === null) {
            return;
        }
        setOperation("stopping");
        setOperationError(null);
        try {
            const status = await api.stopTerminal(selectedSessionId);
            cacheTerminalStatus(selectedSessionId, status);
            setAttachment(null);
            setConnectionState("exited");
            setCopyMode(false);
        } catch (error) {
            setOperationError(errorMessage(error));
        } finally {
            setOperation(null);
        }
    }

    async function handleCopyMode() {
        if (selectedSessionId === null || attachment === null) {
            return;
        }
        setOperationError(null);
        try {
            const status = await api.enterTerminalCopyMode(
                selectedSessionId,
                attachment.attachmentId,
            );
            cacheTerminalStatus(selectedSessionId, status);
            setCopyMode(true);
        } catch (error) {
            setOperationError(errorMessage(error));
        }
    }

    async function handleSaveWorkspace(sessionId: string) {
        const details = promptWorkspaceSaveDetails();
        if (details === null) {
            return;
        }
        setSaving(true);
        setOperationError(null);
        try {
            await api.saveSessionWorkspace(sessionId, details);
            await queryClient.invalidateQueries({ queryKey: queryKeys.workspaces });
            window.alert(`Workspace "${details.name}" saved.`);
        } catch (error) {
            setOperationError(errorMessage(error));
        } finally {
            setSaving(false);
        }
    }

    async function handleDeleteSession(sessionId: string) {
        const decision = promptSaveWorkspace();
        if (decision.action === "cancel") {
            return;
        }
        setDeleting(true);
        setOperationError(null);
        try {
            if (decision.action === "save") {
                await api.saveSessionWorkspace(sessionId, decision);
                await queryClient.invalidateQueries({ queryKey: queryKeys.workspaces });
            }
            await api.deleteSession(sessionId);
            queryClient.removeQueries({ queryKey: queryKeys.session(sessionId) });
            queryClient.removeQueries({ queryKey: queryKeys.terminal(sessionId) });
            await queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            await queryClient.invalidateQueries({ queryKey: queryKeys.kernels });
            if (selectedSessionId === sessionId) {
                selectSession(null);
            }
        } catch (error) {
            setOperationError(errorMessage(error));
        } finally {
            setDeleting(false);
        }
    }

    const agentName = selectedSession === undefined
        ? ""
        : (agentMap.get(selectedSession.agent_id)?.name ?? selectedSession.agent_id);
    const displayStatus = selectedSession === undefined
        ? "disconnected"
        : runtimeLabel(selectedSession, terminalStatus, connectionState, operation);
    const displayAttachKind = attachKindLabel(attachKind);
    const busy = creating || operation !== null || saving || deleting;
    const canReconnect = selectedSession !== undefined
        && selectedSession.recovery_state !== "legacy-unrecoverable"
        && (
            terminalStatus?.state !== "running"
            || connectionState === "disconnected"
            || connectionState === "error"
        );

    return (
        <div className="cli-layout">
            <aside className="session-rail">
                <div className="session-rail-header">
                    <h2>CLI sessions</h2>
                    <Tooltip content="New CLI session" relationship="label">
                        <Button
                            appearance="subtle"
                            icon={<Add20Regular />}
                            onClick={() => setShowNewSession(true)}
                            size="small"
                        />
                    </Tooltip>
                </div>
                <div aria-label="CLI sessions" className="session-list">
                    {sessions.map((session) => (
                        <div
                            className={`session-row${
                                selectedSessionId === session.session_id ? " active" : ""
                            }`}
                            key={session.session_id}
                        >
                            <button
                                aria-current={selectedSessionId === session.session_id}
                                className="session-row-button cli-session-row-button"
                                onClick={() => selectSession(session.session_id)}
                                title={session.session_id}
                                type="button"
                            >
                                <span className="session-row-title">
                                    <span
                                        aria-hidden="true"
                                        className={`status-dot ${
                                            statusTone(session.runtime_status ?? session.status)
                                        }`}
                                    />
                                    <span className="truncate">
                                        {agentMap.get(session.agent_id)?.name ?? session.agent_id}
                                    </span>
                                </span>
                                <span className="session-row-meta">
                                    <span>{session.runtime_status ?? session.status}</span>
                                    <span aria-hidden="true">·</span>
                                    <span className="mono">{compactId(session.session_id)}</span>
                                </span>
                            </button>
                            <Tooltip content="Delete session" relationship="description">
                                <Button
                                    appearance="subtle"
                                    aria-label={`Delete ${
                                        agentMap.get(session.agent_id)?.name ?? session.agent_id
                                    } CLI session`}
                                    className="session-row-delete"
                                    disabled={busy}
                                    icon={<Delete20Regular />}
                                    onClick={() => void handleDeleteSession(session.session_id)}
                                    size="small"
                                />
                            </Tooltip>
                        </div>
                    ))}
                    {sessions.length === 0 && (
                        <p className="session-list-empty">
                            No CLI sessions yet. Start one with a CLI-enabled agent.
                        </p>
                    )}
                </div>
            </aside>

            <section className="cli-main">
                {selectedSession !== undefined && (
                    <header className="chat-header cli-header">
                        <div className="chat-header-title">
                            <div className="chat-header-heading">
                                <h2>{agentName}</h2>
                                <StatusBadge label={displayStatus} tone={statusTone(displayStatus)} />
                                {displayAttachKind !== null && (
                                    <span className={`attach-kind ${attachKind ?? ""}`}>
                                        {displayAttachKind}
                                    </span>
                                )}
                            </div>
                            <div className="chat-header-meta">
                                <span title={selectedSession.session_id}>
                                    AgentSpace {compactId(selectedSession.session_id)}
                                </span>
                                <span aria-hidden="true">·</span>
                                <span title={selectedSession.harness_session_id ?? undefined}>
                                    Copilot {selectedSession.harness_session_id
                                        ? compactId(selectedSession.harness_session_id)
                                        : "not assigned"}
                                </span>
                                <span aria-hidden="true">·</span>
                                <span>
                                    {terminalStatus?.attachment_count ?? 0} attachment{
                                        terminalStatus?.attachment_count === 1 ? "" : "s"
                                    }
                                </span>
                                <details className="cli-details">
                                    <summary>Details</summary>
                                    <dl>
                                        <dt>AgentSpace session</dt>
                                        <dd>{selectedSession.session_id}</dd>
                                        <dt>Copilot session</dt>
                                        <dd>{selectedSession.harness_session_id ?? "Not assigned"}</dd>
                                        <dt>CLI harness</dt>
                                        <dd>{selectedSession.cli_harness ?? "Unknown"}</dd>
                                        <dt>Runtime generation</dt>
                                        <dd>{selectedSession.runtime_generation ?? "—"}</dd>
                                    </dl>
                                </details>
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
                            {canReconnect && (
                                <Button
                                    disabled={busy}
                                    icon={<ArrowClockwise20Regular />}
                                    onClick={() => void handleReconnect()}
                                    size="small"
                                >
                                    {terminalStatus?.state === "exited" ? "Resume" : "Reconnect"}
                                </Button>
                            )}
                            <Tooltip
                                content={attachment === null
                                    ? "Attach the terminal before entering scrollback"
                                    : "Enter tmux scrollback for this attachment"}
                                relationship="description"
                            >
                                <Button
                                    disabled={busy || attachment === null}
                                    icon={<History20Regular />}
                                    onClick={() => void handleCopyMode()}
                                    size="small"
                                >
                                    Scrollback
                                </Button>
                            </Tooltip>
                            <Menu positioning="below-end">
                                <MenuTrigger disableButtonEnhancement>
                                    <Tooltip content="CLI session actions" relationship="label">
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
                                            disabled={busy || selectedKernel === null}
                                            icon={<Save20Regular />}
                                            onClick={() =>
                                                void handleSaveWorkspace(selectedSession.session_id)}
                                        >
                                            Save workspace
                                        </MenuItem>
                                        <MenuItem
                                            disabled={busy || terminalStatus?.state !== "running"}
                                            icon={<Stop20Regular />}
                                            onClick={() => void handleStop()}
                                        >
                                            Stop CLI
                                        </MenuItem>
                                        <MenuItem
                                            disabled={busy}
                                            icon={<Delete20Regular />}
                                            onClick={() =>
                                                void handleDeleteSession(selectedSession.session_id)}
                                            style={{ color: "var(--danger)" }}
                                        >
                                            Delete session
                                        </MenuItem>
                                    </MenuList>
                                </MenuPopover>
                            </Menu>
                        </div>
                    </header>
                )}

                {operationError !== null && (
                    <div className="cli-error" role="alert">
                        {operationError}
                    </div>
                )}
                {copyMode && (
                    <div className="copy-mode-notice" role="status">
                        Scrollback is active for this attachment. Press <kbd>q</kbd> to exit copy
                        mode and return mouse input to Copilot.
                    </div>
                )}

                {selectedSessionId === null
                    ? (
                        <div className="cli-placeholder">
                            <EmptyState
                                action={(
                                    <Button
                                        appearance="primary"
                                        icon={<Add20Regular />}
                                        onClick={() => setShowNewSession(true)}
                                    >
                                        New CLI session
                                    </Button>
                                )}
                                description="Select a session or start Copilot in a CLI-enabled agent."
                                icon={<WindowConsole20Regular />}
                                title="Open an interactive terminal"
                            />
                        </div>
                    )
                    : selectedSessionQuery.isLoading
                    ? <LoadingState label="Loading CLI session…" />
                    : selectedSessionQuery.isError
                    ? (
                        <div className="cli-placeholder">
                            <EmptyState
                                description={errorMessage(selectedSessionQuery.error)}
                                title="Could not load CLI session"
                            />
                        </div>
                    )
                    : selectedSession?.recovery_state === "legacy-unrecoverable"
                    ? (
                        <div className="cli-placeholder">
                            <EmptyState
                                description="This legacy session cannot recreate a lost runtime. You can delete it or use the original live runtime while it remains available."
                                title="Legacy recovery unavailable"
                            />
                        </div>
                    )
                    : operation === "starting" && terminalStatus === null
                    ? <LoadingState label="Starting CLI terminal…" />
                    : terminalStatus?.state === "running"
                    ? (
                        <Terminal
                            darkMode={darkMode}
                            onAttachmentChange={setAttachment}
                            onConnectionStateChange={setConnectionState}
                            onLifecycleStatus={handleLifecycleStatus}
                            reconnectKey={reconnectKey}
                            sessionId={selectedSessionId}
                        />
                    )
                    : terminalStatus?.state === "exited"
                    ? (
                        <div className="cli-placeholder">
                            <EmptyState
                                action={(
                                    <Button
                                        appearance="primary"
                                        disabled={busy}
                                        icon={<ArrowClockwise20Regular />}
                                        onClick={() => void handleReconnect()}
                                    >
                                        Resume Copilot
                                    </Button>
                                )}
                                description={terminalStatus.exit_status === null
                                    ? "The terminal pane exited. Resume it with the same Copilot session and workspace."
                                    : `The terminal pane exited with status ${
                                        terminalStatus.exit_status
                                    }. Resume it with the same Copilot session and workspace.`}
                                title="CLI stopped"
                            />
                        </div>
                    )
                    : (
                        <div className="cli-placeholder">
                            <EmptyState
                                action={(
                                    <Button
                                        appearance="primary"
                                        disabled={busy}
                                        icon={<ArrowClockwise20Regular />}
                                        onClick={() => void handleReconnect()}
                                    >
                                        Reconnect
                                    </Button>
                                )}
                                description={operationError
                                    ?? errorMessage(
                                        terminalStatusQuery.error
                                        ?? "The terminal is not currently attached.",
                                    )}
                                title="Terminal unavailable"
                            />
                        </div>
                    )}
            </section>

            <FormDialog
                busy={creating || cliAgents.length === 0}
                onOpenChange={setShowNewSession}
                onSubmit={() => void handleCreateSession()}
                open={showNewSession}
                submitLabel="Start CLI session"
                title="New CLI session"
            >
                {cliAgents.length === 0
                    ? (
                        <p className="muted">
                            No agents have CLI capability enabled. Enable CLI sessions on an agent
                            first.
                        </p>
                    )
                    : (
                        <Field label="Agent" required>
                            <Select
                                onChange={(event) => setNewSessionAgentId(event.target.value)}
                                value={newSessionAgentId || cliAgents[0]?.agent_id}
                            >
                                {cliAgents.map((agent) => (
                                    <option key={agent.agent_id} value={agent.agent_id}>
                                        {agent.name} ({agent.agent_id})
                                    </option>
                                ))}
                            </Select>
                        </Field>
                    )}
            </FormDialog>
        </div>
    );
}
