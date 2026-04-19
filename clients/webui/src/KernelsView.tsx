import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import Editor, { type OnMount } from "@monaco-editor/react";
import type { editor } from "monaco-editor";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { queryKeys, useKernels } from "./queries";
import { useErrorContext } from "./ErrorContext";
import type { KernelStats, KernelSummary } from "./types";

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
            reportError(err as Error);
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
        <div className="view-content">
            <div className="view-header">
                <h2>Running Kernels</h2>
                <span className="muted">{kernels.length} active</span>
            </div>

            {kernels.length > 0 ? (
                <div className="table-container">
                    <table className="data-table kernels-table">
                        <thead>
                            <tr>
                                <th>Harness</th>
                                <th>Session</th>
                                <th>Container</th>
                                <th>Status</th>
                                <th className="num">CPU</th>
                                <th className="num">Memory</th>
                                <th className="num">Turns</th>
                                <th>Clients</th>
                                <th aria-label="Actions"></th>
                            </tr>
                        </thead>
                        <tbody>
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
                                        killMutation.mutate(kernel.session_id);
                                    }}
                                    killDisabled={killMutation.isPending}
                                />
                            ))}
                        </tbody>
                    </table>
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
                                    <select
                                        value={logsState.source}
                                        onChange={(e) =>
                                            setLogSource(e.target.value as LogSource)
                                        }
                                    >
                                        <option value="harness">Harness logs</option>
                                        <option value="container">Container logs</option>
                                    </select>
                                </label>
                                <label className="muted small follow-toggle">
                                    <input
                                        type="checkbox"
                                        checked={follow}
                                        onChange={(e) => setFollow(e.target.checked)}
                                    />
                                    Follow
                                </label>
                                <span className="muted small">
                                    {loadingLogs ? "Loading…" : ""}
                                </span>
                                <button
                                    className="secondary-button small"
                                    onClick={() => void downloadAllLogs()}
                                    type="button"
                                >
                                    Download all
                                </button>
                                <button
                                    className="secondary-button small"
                                    onClick={closeLogs}
                                    type="button"
                                >
                                    Close
                                </button>
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

    useEffect(() => {
        if (!isMenuOpen) {
            setMenuPos(null);
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
        <tr>
            <td>{kernel.harness}</td>
            <td className="mono">{kernel.session_id.slice(0, 12)}…</td>
            <td className="mono">{kernel.container_name ?? "—"}</td>
            <td>
                <span className={`status-badge ${kernel.status}`}>{kernel.status}</span>
            </td>
            <td className="num mono">{formatCpu(kernel.stats)}</td>
            <td className="num mono">{formatMemory(kernel.stats)}</td>
            <td className="num">{kernel.turns}</td>
            <td>
                {kernel.client_session_ids.length > 0 ? (
                    <span className="muted small">
                        {kernel.client_session_ids.length} attached
                    </span>
                ) : (
                    <span className="muted small">—</span>
                )}
            </td>
            <td className="actions-cell">
                <button
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
                </button>
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
                            <button
                                type="button"
                                role="menuitem"
                                className="kebab-menu-item"
                                onClick={onViewLogs}
                            >
                                View logs
                            </button>
                            <button
                                type="button"
                                role="menuitem"
                                className="kebab-menu-item danger"
                                disabled={killDisabled}
                                onClick={onKill}
                            >
                                Kill
                            </button>
                        </div>,
                        document.body,
                    )}
            </td>
        </tr>
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
