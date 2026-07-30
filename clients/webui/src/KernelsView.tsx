import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import Editor, { type OnMount } from "@monaco-editor/react";
import type { editor } from "monaco-editor";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { browserReachableLocalUrl } from "./browserUrls";
import { queryKeys, useKernels } from "./queries";
import { useErrorContext } from "./useErrorContext";
import type { KernelStats, KernelSummary } from "./types";
import { promptSaveWorkspace } from "./saveWorkspacePrompt";
import {
    Button,
    Checkbox,
    Select,
    Table,
    TableBody,
    TableCell,
    TableHeader,
    TableHeaderCell,
    TableRow,
} from "./fluent";

const LOG_POLL_INTERVAL_MS = 1000;
const DEFAULT_LOG_TAIL = 2000;

type LogSource = "harness" | "container";

type LogsModalState = {
    sessionId: string;
    source: LogSource;
};

export default function KernelsView() {
    const { data: kernels = [] } = useKernels();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [logsState, setLogsState] = useState<LogsModalState | null>(null);
    const [follow, setFollow] = useState(true);
    const [openMenuFor, setOpenMenuFor] = useState<string | null>(null);
    const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null);
    const followRef = useRef(follow);

    const killMutation = useMutation({
        mutationFn: (sessionId: string) => api.killKernel(sessionId),
        onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.kernels }),
        onError: reportError,
    });
    const deleteSessionMutation = useMutation({
        mutationFn: async (sessionIds: string[]) => {
            for (const sessionId of sessionIds) {
                await api.deleteSession(sessionId);
            }
        },
        onSuccess: () => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            void queryClient.invalidateQueries({ queryKey: queryKeys.kernels });
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

    const harnessLogsQuery = useQuery({
        queryKey: logsState
            ? queryKeys.kernelLogs(logsState.sessionId)
            : (["kernels", "__none__", "logs"] as const),
        queryFn: () => api.kernelLogs(logsState!.sessionId),
        enabled: logsState?.source === "harness",
        refetchInterval: LOG_POLL_INTERVAL_MS,
    });

    const containerLogsQuery = useQuery({
        queryKey: logsState
            ? queryKeys.kernelContainerLogs(logsState.sessionId)
            : (["kernels", "__none__", "container-logs"] as const),
        queryFn: () =>
            api.kernelContainerLogs(logsState!.sessionId, DEFAULT_LOG_TAIL),
        enabled: logsState?.source === "container",
        refetchInterval: LOG_POLL_INTERVAL_MS,
    });

    const activeQuery =
        logsState?.source === "container" ? containerLogsQuery : harnessLogsQuery;
    const logLines = useMemo(
        () => activeQuery.data?.lines ?? [],
        [activeQuery.data],
    );

    useEffect(() => {
        followRef.current = follow;
    }, [follow]);

    function scrollToBottom() {
        const ed = editorRef.current;
        if (!ed) {
            return;
        }
        ed.setScrollPosition({ scrollTop: ed.getScrollHeight() });
    }

    useEffect(() => {
        if (followRef.current) {
            scrollToBottom();
        }
    }, [logLines]);

    useEffect(() => {
        if (follow) {
            scrollToBottom();
        }
    }, [follow]);

    function closeLogs() {
        setLogsState(null);
        editorRef.current = null;
    }

    useEffect(() => {
        if (logsState === null) {
            return;
        }
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                closeLogs();
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [logsState]);

    useEffect(() => {
        if (openMenuFor === null) {
            return;
        }
        const onClick = () => setOpenMenuFor(null);
        const handle = window.setTimeout(() => {
            window.addEventListener("click", onClick);
        }, 0);
        return () => {
            window.clearTimeout(handle);
            window.removeEventListener("click", onClick);
        };
    }, [openMenuFor]);

    const handleEditorMount: OnMount = (editorInstance) => {
        editorRef.current = editorInstance;
        if (followRef.current) {
            scrollToBottom();
        }
    };

    function openLogs(sessionId: string, source: LogSource = "harness") {
        setLogsState({ sessionId, source });
        setFollow(true);
    }

    function setLogSource(source: LogSource) {
        if (!logsState) return;
        setLogsState({ ...logsState, source });
        setFollow(true);
    }

    async function handleKillKernel(kernel: KernelSummary) {
        if (kernel.client_session_ids.length === 0) {
            if (window.confirm("Kill this orphan kernel? Its /workspace volume will be destroyed.")) {
                killMutation.mutate(kernel.session_id);
            }
            return;
        }
        const decision = promptSaveWorkspace();
        if (decision.action === "cancel") {
            return;
        }
        if (decision.action === "save") {
            try {
                await saveWorkspaceMutation.mutateAsync({
                    sessionId: kernel.client_session_ids[0],
                    ...decision,
                });
            } catch {
                return;
            }
        }
        deleteSessionMutation.mutate(kernel.client_session_ids);
    }

    async function downloadAllLogs() {
        if (!logsState) return;
        try {
            const result =
                logsState.source === "container"
                    ? await api.kernelContainerLogs(logsState.sessionId, "all")
                    : await api.kernelLogs(logsState.sessionId);
            const blob = new Blob([result.lines.join("\n")], {
                type: "text/plain;charset=utf-8",
            });
            const url = URL.createObjectURL(blob);
            const link = document.createElement("a");
            link.href = url;
            link.download = `kernel-${logsState.sessionId.slice(0, 12)}-${logsState.source}.txt`;
            document.body.appendChild(link);
            link.click();
            document.body.removeChild(link);
            URL.revokeObjectURL(url);
        } catch (err) {
            reportError(err);
        }
    }

    const editorTheme =
        document.documentElement.getAttribute("data-theme") === "dark" ? "vs-dark" : "light";

    const loadingLogs = activeQuery.isFetching && activeQuery.isLoading;
    const tailNote =
        logsState?.source === "container"
            ? `Last ${DEFAULT_LOG_TAIL.toLocaleString()} lines · auto-refresh 1s`
            : "Auto-refresh 1s";

    return (
        <div className="view-content management-view kernels-management-view">
            <div className="view-header">
                <div>
                    <h2>Running Kernels</h2>
                    <span className="muted">
                        {kernels.length} active · {kernels.reduce((total, kernel) => total + kernel.turns, 0)} turns
                    </span>
                </div>
            </div>

            {kernels.length > 0 ? (
                <div className="table-container management-table-container">
                    <Table className="data-table management-table kernels-table">
                        <TableHeader>
                            <TableRow>
                                <TableHeaderCell>Harness</TableHeaderCell>
                                <TableHeaderCell>Session</TableHeaderCell>
                                <TableHeaderCell>Container</TableHeaderCell>
                                <TableHeaderCell>Status</TableHeaderCell>
                                <TableHeaderCell className="num">CPU</TableHeaderCell>
                                <TableHeaderCell className="num">Memory</TableHeaderCell>
                                <TableHeaderCell className="num">Turns</TableHeaderCell>
                                <TableHeaderCell>Clients</TableHeaderCell>
                                <TableHeaderCell aria-label="Actions"></TableHeaderCell>
                            </TableRow>
                        </TableHeader>
                        <TableBody>
                            {kernels.map((kernel) => (
                                <KernelRow
                                    key={kernel.session_id}
                                    kernel={kernel}
                                    isMenuOpen={openMenuFor === kernel.session_id}
                                    onToggleMenu={() =>
                                        setOpenMenuFor((current) =>
                                            current === kernel.session_id
                                                ? null
                                                : kernel.session_id,
                                        )
                                    }
                                    onViewLogs={() => {
                                        setOpenMenuFor(null);
                                        openLogs(kernel.session_id);
                                    }}
                                    onKill={() => {
                                        setOpenMenuFor(null);
                                        void handleKillKernel(kernel);
                                    }}
                                    killDisabled={
                                        killMutation.isPending
                                        || deleteSessionMutation.isPending
                                        || saveWorkspaceMutation.isPending
                                    }
                                />
                            ))}
                        </TableBody>
                    </Table>
                </div>
            ) : (
                <div className="empty-state">No active kernels.</div>
            )}

            {logsState && (
                <div
                    className="tool-detail-overlay"
                    onClick={(e) => {
                        if (e.target === e.currentTarget) {
                            closeLogs();
                        }
                    }}
                >
                    <div className="logs-modal">
                        <div className="logs-modal-header">
                            <div className="logs-modal-title">
                                <h3>Kernel logs — {logsState.sessionId.slice(0, 12)}…</h3>
                                <span className="muted small">{tailNote}</span>
                            </div>
                            <div className="logs-modal-actions">
                                <label className="muted small log-source-select">
                                    Source:&nbsp;
                                    <Select
                                        value={logsState.source}
                                        onChange={(e) =>
                                            setLogSource(e.target.value as LogSource)
                                        }
                                    >
                                        <option value="harness">Harness logs</option>
                                        <option value="container">Container logs</option>
                                    </Select>
                                </label>
                                <Checkbox
                                    checked={follow}
                                    className="muted small follow-toggle"
                                    label="Follow"
                                    onChange={(_, data) => setFollow(data.checked === true)}
                                />
                                <span className="muted small">
                                    {loadingLogs ? "Loading…" : ""}
                                </span>
                                <Button
                                    className="secondary-button small"
                                    onClick={() => void downloadAllLogs()}
                                    type="button"
                                >
                                    Download all
                                </Button>
                                <Button
                                    className="secondary-button small"
                                    onClick={closeLogs}
                                    type="button"
                                >
                                    Close
                                </Button>
                            </div>
                        </div>
                        <div className="logs-modal-body">
                            <Editor
                                height="100%"
                                language="log"
                                value={logLines.length > 0 ? logLines.join("\n") : "(no logs yet)"}
                                theme={editorTheme}
                                onMount={handleEditorMount}
                                options={{
                                    readOnly: true,
                                    domReadOnly: true,
                                    minimap: { enabled: false },
                                    lineNumbers: "on",
                                    scrollBeyondLastLine: false,
                                    wordWrap: "on",
                                    fontSize: 12,
                                    automaticLayout: true,
                                    fixedOverflowWidgets: true,
                                    renderLineHighlight: "none",
                                }}
                            />
                        </div>
                    </div>
                </div>
            )}
        </div>
    );
}

type KernelRowProps = {
    kernel: KernelSummary;
    isMenuOpen: boolean;
    onToggleMenu: () => void;
    onViewLogs: () => void;
    onKill: () => void;
    killDisabled: boolean;
};

function KernelRow({
    kernel,
    isMenuOpen,
    onToggleMenu,
    onViewLogs,
    onKill,
    killDisabled,
}: KernelRowProps) {
    const buttonRef = useRef<HTMLButtonElement | null>(null);
    const [menuPos, setMenuPos] = useState<{ top: number; right: number } | null>(
        null,
    );

    // The menu is only rendered while `isMenuOpen`, so a stale position while
    // closed is harmless — it is recomputed before the menu is shown again.
    useEffect(() => {
        if (!isMenuOpen) {
            return;
        }
        const update = () => {
            const btn = buttonRef.current;
            if (!btn) return;
            const rect = btn.getBoundingClientRect();
            setMenuPos({
                top: rect.bottom + 4,
                right: window.innerWidth - rect.right,
            });
        };
        update();
        window.addEventListener("scroll", update, true);
        window.addEventListener("resize", update);
        return () => {
            window.removeEventListener("scroll", update, true);
            window.removeEventListener("resize", update);
        };
    }, [isMenuOpen]);

    return (
        <TableRow>
            <TableCell>
                <span className="tag">{kernel.harness}</span>
            </TableCell>
            <TableCell className="mono" title={kernel.session_id}>
                <span className="truncate-value">{kernel.session_id.slice(0, 12)}…</span>
            </TableCell>
            <TableCell className="mono" title={kernel.container_name ?? undefined}>
                <span className="truncate-value">{kernel.container_name ?? "—"}</span>
            </TableCell>
            <TableCell>
                <span className={`status-badge ${kernel.status}`}>{kernel.status}</span>
            </TableCell>
            <TableCell className="num mono">{formatCpu(kernel.stats)}</TableCell>
            <TableCell className="num mono">{formatMemory(kernel.stats)}</TableCell>
            <TableCell className="num">{kernel.turns}</TableCell>
            <TableCell>
                {kernel.client_session_ids.length > 0 ? (
                    <span className="muted small">
                        {kernel.client_session_ids.length} attached
                    </span>
                ) : (
                    <span className="muted small">—</span>
                )}
            </TableCell>
            <TableCell className="actions-cell">
                <Button
                    ref={buttonRef}
                    type="button"
                    className="kebab-button"
                    aria-haspopup="menu"
                    aria-expanded={isMenuOpen}
                    aria-label="Actions"
                    onClick={(e) => {
                        e.stopPropagation();
                        onToggleMenu();
                    }}
                >
                    ⋯
                </Button>
                {isMenuOpen &&
                    menuPos !== null &&
                    createPortal(
                        <div
                            className="kebab-menu"
                            role="menu"
                            style={{
                                position: "fixed",
                                top: menuPos.top,
                                right: menuPos.right,
                            }}
                            onClick={(e) => e.stopPropagation()}
                        >
                            <Button
                                type="button"
                                role="menuitem"
                                className="kebab-menu-item"
                                onClick={onViewLogs}
                            >
                                View logs
                            </Button>
                            {kernel.vscode_url ? (
                                <a
                                    role="menuitem"
                                    className="kebab-menu-item"
                                    href={browserReachableLocalUrl(kernel.vscode_url)}
                                    target="_blank"
                                    rel="noreferrer"
                                    onClick={onToggleMenu}
                                >
                                    Open VS Code
                                </a>
                            ) : (
                                <Button
                                    type="button"
                                    role="menuitem"
                                    className="kebab-menu-item"
                                    disabled
                                >
                                    VS Code unavailable
                                </Button>
                            )}
                            {kernel.free_port_url ? (
                                <a
                                    role="menuitem"
                                    className="kebab-menu-item"
                                    href={browserReachableLocalUrl(kernel.free_port_url)}
                                    target="_blank"
                                    rel="noreferrer"
                                    onClick={onToggleMenu}
                                >
                                    Open service
                                </a>
                            ) : null}
                            <Button
                                type="button"
                                role="menuitem"
                                className="kebab-menu-item danger"
                                disabled={killDisabled}
                                onClick={onKill}
                            >
                                Kill
                            </Button>
                        </div>,
                        document.body,
                    )}
            </TableCell>
        </TableRow>
    );
}

function formatCpu(stats: KernelStats | null): string {
    if (!stats || stats.cpu_percent === null) {
        return "—";
    }
    return `${stats.cpu_percent.toFixed(1)}%`;
}

function formatMemory(stats: KernelStats | null): string {
    if (!stats || stats.memory_usage_bytes === null) {
        return "—";
    }
    const used = formatBytes(stats.memory_usage_bytes);
    if (stats.memory_limit_bytes === null || stats.memory_limit_bytes <= 0) {
        return used;
    }
    const limit = formatBytes(stats.memory_limit_bytes);
    const pct =
        stats.memory_percent !== null ? ` (${stats.memory_percent.toFixed(1)}%)` : "";
    return `${used} / ${limit}${pct}`;
}

function formatBytes(bytes: number): string {
    if (bytes < 1024) {
        return `${bytes} B`;
    }
    const units = ["KiB", "MiB", "GiB", "TiB"];
    let value = bytes / 1024;
    let unitIndex = 0;
    while (value >= 1024 && unitIndex < units.length - 1) {
        value /= 1024;
        unitIndex += 1;
    }
    const precision = value >= 100 ? 0 : value >= 10 ? 1 : 2;
    return `${value.toFixed(precision)} ${units[unitIndex]}`;
}
