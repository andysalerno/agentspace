import { useState } from "react";
import { api } from "./api";
import type { KernelSummary } from "./types";

type KernelsViewProps = {
    kernels: KernelSummary[];
    onKillKernel: (sessionId: string) => void;
    busy: boolean;
};

export default function KernelsView({ kernels, onKillKernel, busy }: KernelsViewProps) {
    const [logsFor, setLogsFor] = useState<string | null>(null);
    const [logLines, setLogLines] = useState<string[]>([]);
    const [loadingLogs, setLoadingLogs] = useState(false);

    async function fetchLogs(sessionId: string) {
        setLoadingLogs(true);
        try {
            const data = await api.kernelLogs(sessionId);
            setLogLines(data.lines);
            setLogsFor(sessionId);
        } finally {
            setLoadingLogs(false);
        }
    }

    function closeLogs() {
        setLogsFor(null);
        setLogLines([]);
    }

    return (
        <div className="view-content">
            <div className="view-header">
                <h2>Kernels</h2>
                <span className="muted">{kernels.length} active</span>
            </div>

            {logsFor && (
                <div className="kernel-logs-panel card">
                    <div className="kernel-logs-header">
                        <h3>Logs — {logsFor.slice(0, 12)}…</h3>
                        <div className="card-footer-actions">
                            <button
                                className="secondary-button small"
                                disabled={loadingLogs}
                                onClick={() => fetchLogs(logsFor)}
                                type="button"
                            >
                                {loadingLogs ? "Loading…" : "Refresh"}
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
                    <pre className="kernel-logs-content">
                        {logLines.length > 0
                            ? logLines.join("\n")
                            : "(no logs yet)"}
                    </pre>
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
                                        onClick={() => fetchLogs(kernel.session_id)}
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
