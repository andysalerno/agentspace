import { useEffect, useRef, useState } from "react";
import Editor, { type OnMount } from "@monaco-editor/react";
import type { editor } from "monaco-editor";
import { api } from "./api";
import type { KernelSummary } from "./types";

type KernelsViewProps = {
    kernels: KernelSummary[];
    onKillKernel: (sessionId: string) => void;
    busy: boolean;
};

const LOG_POLL_INTERVAL_MS = 1000;

export default function KernelsView({ kernels, onKillKernel, busy }: KernelsViewProps) {
    const [logsFor, setLogsFor] = useState<string | null>(null);
    const [logLines, setLogLines] = useState<string[]>([]);
    const [loadingLogs, setLoadingLogs] = useState(false);
    const [follow, setFollow] = useState(true);
    const logsForRef = useRef<string | null>(null);
    const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null);
    const followRef = useRef(follow);

    useEffect(() => {
        logsForRef.current = logsFor;
    }, [logsFor]);

    useEffect(() => {
        followRef.current = follow;
    }, [follow]);

    function scrollToBottom() {
        const ed = editorRef.current;
        if (!ed) {
            return;
        }
        // Use scroll position rather than revealLine: with wordWrap="on",
        // a single model line can span many visual rows, and revealLine only
        // guarantees the model line is visible -- not the final wrapped row.
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

    const handleEditorMount: OnMount = (editorInstance) => {
        editorRef.current = editorInstance;
        if (followRef.current) {
            scrollToBottom();
        }
    };

    async function fetchLogs(sessionId: string, showSpinner: boolean) {
        if (showSpinner) {
            setLoadingLogs(true);
        }
        try {
            const data = await api.kernelLogs(sessionId);
            if (logsForRef.current === sessionId) {
                setLogLines(data.lines);
            }
        } finally {
            if (showSpinner) {
                setLoadingLogs(false);
            }
        }
    }

    async function openLogs(sessionId: string) {
        setLogsFor(sessionId);
        setLogLines([]);
        setFollow(true);
        await fetchLogs(sessionId, true);
    }

    function closeLogs() {
        setLogsFor(null);
        setLogLines([]);
        editorRef.current = null;
    }

    useEffect(() => {
        if (logsFor === null) {
            return;
        }
        const sessionId = logsFor;
        const interval = window.setInterval(() => {
            void fetchLogs(sessionId, false);
        }, LOG_POLL_INTERVAL_MS);
        return () => {
            window.clearInterval(interval);
        };
    }, [logsFor]);

    const editorTheme =
        document.documentElement.getAttribute("data-theme") === "dark" ? "vs-dark" : "light";

    return (
        <div className="view-content">
            <div className="view-header">
                <h2>Running Kernels</h2>
                <span className="muted">{kernels.length} active</span>
            </div>

            {logsFor && (
                <div className="kernel-logs-panel card">
                    <div className="kernel-logs-header">
                        <h3>Logs — {logsFor.slice(0, 12)}…</h3>
                        <div className="card-footer-actions">
                            <label
                                className="muted small"
                                style={{ display: "inline-flex", alignItems: "center", gap: "0.35rem", cursor: "pointer" }}
                            >
                                <input
                                    type="checkbox"
                                    checked={follow}
                                    onChange={(e) => setFollow(e.target.checked)}
                                />
                                Follow
                            </label>
                            <span className="muted small">
                                {loadingLogs ? "Loading…" : "Auto-refresh: 1s"}
                            </span>
                            <button
                                className="secondary-button small"
                                onClick={closeLogs}
                                type="button"
                            >
                                Close
                            </button>
                        </div>
                    </div>
                    <div
                        style={{
                            border: "1px solid var(--border-color)",
                            borderRadius: "var(--radius-sm)",
                            overflow: "hidden",
                        }}
                    >
                        <Editor
                            height="400px"
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
            )}

            {kernels.length > 0 ? (
                <div className="card-grid">
                    {kernels.map((kernel) => (
                        <div className="card" key={kernel.session_id}>
                            <div className="card-body">
                                <h3>{kernel.harness}</h3>
                                <div className="mono muted">{kernel.session_id.slice(0, 8)}…</div>
                                <div className="detail-grid">
                                    <span className="detail-label">Status</span>
                                    <span className={`status-badge ${kernel.status}`}>{kernel.status}</span>
                                    <span className="detail-label">Turns</span>
                                    <span>{kernel.turns}</span>
                                </div>
                                {kernel.client_session_ids.length > 0 && (
                                    <div className="tag-row">
                                        {kernel.client_session_ids.map((id) => (
                                            <span className="tag" key={id}>
                                                {id.slice(0, 8)}…
                                            </span>
                                        ))}
                                    </div>
                                )}
                            </div>
                            <div className="card-footer">
                                <span className="muted">{kernel.turns} turn{kernel.turns !== 1 ? "s" : ""}</span>
                                <div className="card-footer-actions">
                                    <button
                                        className="secondary-button small"
                                        disabled={loadingLogs}
                                        onClick={() => void openLogs(kernel.session_id)}
                                        type="button"
                                    >
                                        View Logs
                                    </button>
                                    <button
                                        className="danger-button small"
                                        disabled={busy}
                                        onClick={() => onKillKernel(kernel.session_id)}
                                        type="button"
                                    >
                                        Kill
                                    </button>
                                </div>
                            </div>
                        </div>
                    ))}
                </div>
            ) : (
                <div className="empty-state">No active kernels.</div>
            )}
        </div>
    );
}
