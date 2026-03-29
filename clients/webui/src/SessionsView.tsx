import type { Agent, SessionSummary } from "./types";

type SessionsViewProps = {
    sessions: SessionSummary[];
    agents: Agent[];
    onNavigateToChat: (sessionId: string) => void;
};

export default function SessionsView({
    sessions,
    agents,
    onNavigateToChat,
}: SessionsViewProps) {
    const agentMap = Object.fromEntries(agents.map((a) => [a.agent_id, a]));

    return (
        <div className="view-content">
            <div className="view-header">
                <h2>Sessions</h2>
                <span className="muted">{sessions.length} total</span>
            </div>

            {sessions.length > 0 ? (
                <div className="table-container">
                    <table className="data-table">
                        <thead>
                            <tr>
                                <th>Agent</th>
                                <th>Session ID</th>
                                <th>Status</th>
                                <th>Messages</th>
                                <th>Channel</th>
                                <th>Created</th>
                                <th></th>
                            </tr>
                        </thead>
                        <tbody>
                            {sessions.map((s) => (
                                <tr key={s.session_id}>
                                    <td>
                                        <strong>{agentMap[s.agent_id]?.name ?? s.agent_id}</strong>
                                    </td>
                                    <td className="mono">{s.session_id.slice(0, 8)}…</td>
                                    <td>
                                        <span className={`status-badge ${s.status}`}>{s.status}</span>
                                    </td>
                                    <td>{s.message_count}</td>
                                    <td>{s.channel_name ?? "—"}</td>
                                    <td>{new Date(s.created_at).toLocaleDateString()}</td>
                                    <td>
                                        <button
                                            className="secondary-button small"
                                            onClick={() => onNavigateToChat(s.session_id)}
                                            type="button"
                                        >
                                            Open Chat
                                        </button>
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
