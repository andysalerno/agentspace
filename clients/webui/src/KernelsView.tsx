import { useEffect, useMemo, useRef, useState } from "react";
import Editor, { type OnMount } from "@monaco-editor/react";
import type { editor } from "monaco-editor";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
    ArrowDownload20Regular,
    Code24Regular,
    Open20Regular,
    Prohibited20Regular,
    TextBulletListLtr20Regular,
} from "@fluentui/react-icons";
import { api } from "./api";
import { browserReachableLocalUrl } from "./browserUrls";
import { queryKeys, useKernels } from "./queries";
import { useErrorContext } from "./useErrorContext";
import type { KernelStats, KernelSummary } from "./types";
import { promptSaveWorkspace } from "./saveWorkspacePrompt";
import {
    Button,
    Checkbox,
    Dialog,
    DialogActions,
    DialogBody,
    DialogContent,
    DialogSurface,
    DialogTitle,
    Select,
} from "./fluent";
import { EmptyState, RowActions, StatusBadge, ViewHeader } from "./ui";
import { statusTone } from "./status";
import { useMonacoTheme } from "./monacoTheme";

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
        mutationFn: (
            { sessionId, workspace_id, name }: {
                sessionId: string;
                workspace_id: string;
                name: string;
            },
        ) => api.saveSessionWorkspace(sessionId, { workspace_id, name }),
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
        queryFn: () => api.kernelContainerLogs(logsState!.sessionId, DEFAULT_LOG_TAIL),
        enabled: logsState?.source === "container",
        refetchInterval: LOG_POLL_INTERVAL_MS,
    });

    const activeQuery = logsState?.source === "container"
        ? containerLogsQuery
        : harnessLogsQuery;
    const logLines = useMemo(() => activeQuery.data?.lines ?? [], [activeQuery.data]);

    useEffect(() => {
        followRef.current = follow;
    }, [follow]);

    function scrollToBottom() {
        const ed = editorRef.current;
        if (!ed) return;
        ed.setScrollPosition({ scrollTop: ed.getScrollHeight() });
    }

    useEffect(() => {
        if (followRef.current) scrollToBottom();
    }, [logLines]);

    useEffect(() => {
        if (follow) scrollToBottom();
    }, [follow]);

    function closeLogs() {
        setLogsState(null);
        editorRef.current = null;
    }

    const handleEditorMount: OnMount = (editorInstance) => {
        editorRef.current = editorInstance;
        if (followRef.current) scrollToBottom();
    };

    function openLogs(sessionId: string) {
        setLogsState({ sessionId, source: "harness" });
        setFollow(true);
    }

    function setLogSource(source: LogSource) {
        if (!logsState) return;
        setLogsState({ ...logsState, source });
        setFollow(true);
    }

    async function handleKillKernel(kernel: KernelSummary) {
        if (kernel.client_session_ids.length === 0) {
            if (
                window.confirm(
                    "Kill this orphan kernel? Its /workspace volume will be destroyed.",
                )
            ) {
                killMutation.mutate(kernel.session_id);
            }
            return;
        }
        const decision = promptSaveWorkspace();
        if (decision.action === "cancel") return;
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
            const result = logsState.source === "container"
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

    const editorTheme = useMonacoTheme();

    const killDisabled = killMutation.isPending
        || deleteSessionMutation.isPending
        || saveWorkspaceMutation.isPending;

    const totalTurns = kernels.reduce((total, kernel) => total + kernel.turns, 0);
    const tailNote = logsState?.source === "container"
        ? `Last ${DEFAULT_LOG_TAIL.toLocaleString()} lines, refreshed every second`
        : "Refreshed every second";
    // A pending first fetch must not read as a kernel that produced no output.
    const logPlaceholder = activeQuery.isLoading
        ? "(loading logs…)"
        : activeQuery.isError
        ? `(could not load logs: ${
            activeQuery.error instanceof Error
                ? activeQuery.error.message
                : String(activeQuery.error)
        })`
        : "(no log output yet)";

    return (
        <div className="view-content">
            <ViewHeader
                description={`${kernels.length} running, ${totalTurns} turns processed`}
                title="Running kernels"
            />
            <div className="view-body">
                {kernels.length === 0
                    ? (
                        <EmptyState
                            description="Kernels are container processes started on demand when a session becomes active."
                            icon={<Code24Regular />}
                            title="No kernels running"
                        />
                    )
                    : (
                        <div className="table-container">
                            <div className="table-scroll">
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th>Kernel</th>
                                            <th>Container</th>
                                            <th>Status</th>
                                            <th className="num">CPU</th>
                                            <th className="num">Memory</th>
                                            <th className="num">Turns</th>
                                            <th className="num">Clients</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {kernels.map((kernel) => (
                                            <tr key={kernel.session_id}>
                                                <td>
                                                    <div className="cell-identity">
                                                        <span className="cell-identity-name">
                                                            {kernel.harness}
                                                        </span>
                                                        <span
                                                            className="cell-identity-id"
                                                            title={kernel.session_id}
                                                        >
                                                            {kernel.session_id.slice(0, 12)}…
                                                        </span>
                                                    </div>
                                                </td>
                                                <td
                                                    className="mono-sm muted"
                                                    title={kernel.container_name ?? undefined}
                                                >
                                                    <span className="truncate">
                                                        {kernel.container_name ?? "—"}
                                                    </span>
                                                </td>
                                                <td>
                                                    <StatusBadge
                                                        label={kernel.status}
                                                        tone={statusTone(kernel.status)}
                                                    />
                                                </td>
                                                <td className="num mono-sm">
                                                    {formatCpu(kernel.stats)}
                                                </td>
                                                <td className="num mono-sm">
                                                    {formatMemory(kernel.stats)}
                                                </td>
                                                <td className="num">{kernel.turns}</td>
                                                <td className="num">
                                                    {kernel.client_session_ids.length}
                                                </td>
                                                <td className="actions-cell">
                                                    <RowActions
                                                        items={[
                                                            {
                                                                key: "vscode",
                                                                label: kernel.vscode_url
                                                                    ? "Open in VS Code"
                                                                    : "VS Code unavailable",
                                                                icon: <Open20Regular />,
                                                                disabled: !kernel.vscode_url,
                                                                onClick: () => {
                                                                    if (!kernel.vscode_url) return;
                                                                    window.open(
                                                                        browserReachableLocalUrl(
                                                                            kernel.vscode_url,
                                                                        ),
                                                                        "_blank",
                                                                        "noopener,noreferrer",
                                                                    );
                                                                },
                                                            },
                                                            ...(kernel.free_port_url
                                                                ? [{
                                                                    key: "service",
                                                                    label: "Open forwarded service",
                                                                    icon: <Open20Regular />,
                                                                    onClick: () => {
                                                                        window.open(
                                                                            browserReachableLocalUrl(
                                                                                kernel.free_port_url!,
                                                                            ),
                                                                            "_blank",
                                                                            "noopener,noreferrer",
                                                                        );
                                                                    },
                                                                }]
                                                                : []),
                                                            {
                                                                key: "kill",
                                                                label: "Kill kernel",
                                                                icon: <Prohibited20Regular />,
                                                                destructive: true,
                                                                disabled: killDisabled,
                                                                onClick: () => {
                                                                    void handleKillKernel(kernel);
                                                                },
                                                            },
                                                        ]}
                                                        primary={{
                                                            key: "logs",
                                                            label: "Logs",
                                                            icon: <TextBulletListLtr20Regular />,
                                                            onClick: () => openLogs(kernel.session_id),
                                                        }}
                                                    />
                                                </td>
                                            </tr>
                                        ))}
                                    </tbody>
                                </table>
                            </div>
                        </div>
                    )}
            </div>

            <Dialog
                modalType="modal"
                onOpenChange={(_, data) => {
                    if (!data.open) closeLogs();
                }}
                open={logsState !== null}
            >
                <DialogSurface className="form-dialog-wide">
                    <DialogBody>
                        <DialogTitle>
                            Kernel logs
                            <span className="muted-sm" style={{ marginLeft: 8 }}>
                                {logsState?.sessionId.slice(0, 12)}…
                            </span>
                        </DialogTitle>
                        <DialogContent>
                            <div className="log-toolbar">
                                <Select
                                    aria-label="Log source"
                                    onChange={(e) => setLogSource(e.target.value as LogSource)}
                                    value={logsState?.source ?? "harness"}
                                >
                                    <option value="harness">Harness logs</option>
                                    <option value="container">Container logs</option>
                                </Select>
                                <Checkbox
                                    checked={follow}
                                    label="Follow"
                                    onChange={(_, data) => setFollow(data.checked === true)}
                                />
                                <span className="muted-sm">{tailNote}</span>
                                <Button
                                    icon={<ArrowDownload20Regular />}
                                    onClick={() => {
                                        void downloadAllLogs();
                                    }}
                                    size="small"
                                    style={{ marginLeft: "auto" }}
                                    type="button"
                                >
                                    Download
                                </Button>
                            </div>
                            <div className="editor-frame" style={{ height: "min(58vh, 520px)" }}>
                                <Editor
                                    height="100%"
                                    language="log"
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
                                    theme={editorTheme}
                                    value={logLines.length > 0
                                        ? logLines.join("\n")
                                        : logPlaceholder}
                                />
                            </div>
                        </DialogContent>
                        <DialogActions>
                            <Button onClick={closeLogs}>Close</Button>
                        </DialogActions>
                    </DialogBody>
                </DialogSurface>
            </Dialog>
        </div>
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
    return `${used} / ${limit}`;
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
