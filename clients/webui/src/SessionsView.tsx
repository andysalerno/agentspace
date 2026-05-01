import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { useErrorContext } from "./ErrorContext";
import { queryKeys, useAgents, useSessions } from "./queries";

type SessionsViewProps = {
    onNavigateToChat: (sessionId: string) => void;
};

export default function SessionsView({ onNavigateToChat }: SessionsViewProps) {
    const { data: sessions = [] } = useSessions();
    const { data: agents = [] } = useAgents();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();
    const agentMap = Object.fromEntries(agents.map((a) => [a.agent_id, a]));
    const deleteMutation = useMutation({
        mutationFn: (sessionId: string) => api.deleteSession(sessionId),
        onSuccess: (_, sessionId) => {
            queryClient.removeQueries({ queryKey: queryKeys.session(sessionId) });
            void queryClient.invalidateQueries({ queryKey: queryKeys.sessions });
            void queryClient.invalidateQueries({ queryKey: queryKeys.kernels });
        },
        onError: reportError,
    });

    function handleDeleteSession(sessionId: string) {
        if (
            !window.confirm(
                "Delete this session from history? This cannot be undone.",
            )
        ) {
            return;
        }
        deleteMutation.mutate(sessionId);
    }

    return (
        <div className="view-content management-view sessions-management-view">
            <div className="view-header">
                <div>
                    <h2>Sessions</h2>
                    <span className="muted">
                        {sessions.length} total · {sessions.filter((s) => s.status === "active").length} active
                    </span>
                </div>
            </div>

            {sessions.length > 0 ? (
                <div className="table-container management-table-container">
                    <table className="data-table management-table">
                        <thead>
                            <tr>
                                <th>Agent</th>
                                <th>Session ID</th>
                                <th>Status</th>
                                <th>Messages</th>
                                <th>Channel</th>
                                <th>Created</th>
                                <th aria-label="Actions"></th>
                            </tr>
                        </thead>
                        <tbody>
                            {sessions.map((s) => (
                                <tr key={s.session_id}>
                                    <td>
                                        <strong className="truncate-value">{agentMap[s.agent_id]?.name ?? s.agent_id}</strong>
                                        <div className="muted mono truncate-value">{s.agent_id}</div>
                                    </td>
                                    <td className="mono" title={s.session_id}>
                                        <span className="truncate-value">{s.session_id.slice(0, 12)}…</span>
                                    </td>
                                    <td>
                                        <span className={`status-badge ${s.status}`}>{s.status}</span>
                                    </td>
                                    <td>{s.message_count}</td>
                                    <td>
                                        <span className="truncate-value">{s.channel_name ?? "—"}</span>
                                    </td>
                                    <td className="nowrap">{new Date(s.created_at).toLocaleString()}</td>
                                    <td className="actions-cell">
                                        <div className="card-footer-actions">
                                            <button
                                                className="secondary-button small"
                                                onClick={() => onNavigateToChat(s.session_id)}
                                                type="button"
                                            >
                                                Open Chat
                                            </button>
                                            <button
                                                className="danger-button small"
                                                disabled={deleteMutation.isPending}
                                                onClick={() => handleDeleteSession(s.session_id)}
                                                type="button"
                                            >
                                                Delete
                                            </button>
                                        </div>
                                    </td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>
            ) : (
                <div className="empty-state">No sessions yet.</div>
            )}
        </div>
    );
}
