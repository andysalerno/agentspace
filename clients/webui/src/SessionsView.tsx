import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { useErrorContext } from "./useErrorContext";
import { queryKeys, useAgents, useSessions } from "./queries";
import { promptSaveWorkspace } from "./saveWorkspacePrompt";
import {
    Button,
    Table,
    TableBody,
    TableCell,
    TableHeader,
    TableHeaderCell,
    TableRow,
} from "./fluent";

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
    const saveWorkspaceMutation = useMutation({
        mutationFn: ({ sessionId, workspace_id, name }: { sessionId: string; workspace_id: string; name: string }) =>
            api.saveSessionWorkspace(sessionId, { workspace_id, name }),
        onSuccess: () => {
            void queryClient.invalidateQueries({ queryKey: queryKeys.workspaces });
        },
        onError: reportError,
    });

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
                    <Table className="data-table management-table">
                        <TableHeader>
                            <TableRow>
                                <TableHeaderCell>Agent</TableHeaderCell>
                                <TableHeaderCell>Session ID</TableHeaderCell>
                                <TableHeaderCell>Status</TableHeaderCell>
                                <TableHeaderCell>Messages</TableHeaderCell>
                                <TableHeaderCell>Channel</TableHeaderCell>
                                <TableHeaderCell>Created</TableHeaderCell>
                                <TableHeaderCell aria-label="Actions"></TableHeaderCell>
                            </TableRow>
                        </TableHeader>
                        <TableBody>
                            {sessions.map((s) => (
                                <TableRow key={s.session_id}>
                                    <TableCell>
                                        <strong className="truncate-value">{agentMap[s.agent_id]?.name ?? s.agent_id}</strong>
                                        <div className="muted mono truncate-value">{s.agent_id}</div>
                                    </TableCell>
                                    <TableCell className="mono" title={s.session_id}>
                                        <span className="truncate-value">{s.session_id.slice(0, 12)}…</span>
                                    </TableCell>
                                    <TableCell>
                                        <span className={`status-badge ${s.status}`}>{s.status}</span>
                                    </TableCell>
                                    <TableCell>{s.message_count}</TableCell>
                                    <TableCell>
                                        <span className="truncate-value">{s.channel_name ?? "—"}</span>
                                    </TableCell>
                                    <TableCell className="nowrap">{new Date(s.created_at).toLocaleString()}</TableCell>
                                    <TableCell className="actions-cell">
                                        <div className="card-footer-actions">
                                            <Button
                                                className="secondary-button small"
                                                onClick={() => onNavigateToChat(s.session_id)}
                                                type="button"
                                            >
                                                Open Chat
                                            </Button>
                                            <Button
                                                className="danger-button small"
                                                disabled={deleteMutation.isPending || saveWorkspaceMutation.isPending}
                                                onClick={() => void handleDeleteSession(s.session_id)}
                                                type="button"
                                            >
                                                Delete
                                            </Button>
                                        </div>
                                    </TableCell>
                                </TableRow>
                            ))}
                        </TableBody>
                    </Table>
                </div>
            ) : (
                <div className="empty-state">No sessions yet.</div>
            )}
        </div>
    );
}
