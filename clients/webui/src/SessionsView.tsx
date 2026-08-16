import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ChatMultiple24Regular, Delete20Regular, Open20Regular } from "@fluentui/react-icons";
import { api } from "./api";
import { useErrorContext } from "./useErrorContext";
import { queryKeys, useAgents, useSessions } from "./queries";
import { promptSaveWorkspace } from "./saveWorkspacePrompt";
import { EmptyState, RowActions, StatusBadge, ViewHeader } from "./ui";
import { sessionTone } from "./status";

type SessionsViewProps = {
    onNavigateToChat: (sessionId: string) => void;
    onNavigateToCli: (sessionId: string) => void;
};

export default function SessionsView(
    { onNavigateToChat, onNavigateToCli }: SessionsViewProps,
) {
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
    const busy = deleteMutation.isPending || saveWorkspaceMutation.isPending;

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

    const activeCount = sessions.filter((s) => s.status === "active").length;

    return (
        <div className="view-content">
            <ViewHeader
                description={`${sessions.length} total, ${activeCount} active`}
                title="Sessions"
            />
            <div className="view-body">
                {sessions.length === 0
                    ? (
                        <EmptyState
                            description="Sessions appear here once an agent starts a conversation."
                            icon={<ChatMultiple24Regular />}
                            title="No sessions yet"
                        />
                    )
                    : (
                        <div className="table-container">
                            <div className="table-scroll">
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th>Agent</th>
                                            <th>Session</th>
                                            <th>Status</th>
                                            <th>Mode</th>
                                            <th className="num">Messages</th>
                                            <th>Channel</th>
                                            <th>Created</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {sessions.map((s) => (
                                            <tr key={s.session_id}>
                                                <td>
                                                    <div className="cell-identity">
                                                        <span className="cell-identity-name">
                                                            {agentMap[s.agent_id]?.name ?? s.agent_id}
                                                        </span>
                                                        <span className="cell-identity-id">{s.agent_id}</span>
                                                    </div>
                                                </td>
                                                <td className="mono-sm" title={s.session_id}>
                                                    {s.session_id.slice(0, 12)}…
                                                </td>
                                                <td>
                                                    <StatusBadge
                                                        label={s.status}
                                                        tone={sessionTone(s.status)}
                                                    />
                                                </td>
                                                <td>{s.interaction_mode === "cli" ? "CLI" : "Chat"}</td>
                                                <td className="num">{s.message_count}</td>
                                                <td>{s.channel_name ?? "—"}</td>
                                                <td className="nowrap muted">
                                                    {new Date(s.created_at).toLocaleDateString()}
                                                </td>
                                                <td className="actions-cell">
                                                    <RowActions
                                                        items={[{
                                                            key: "delete",
                                                            label: "Delete session",
                                                            icon: <Delete20Regular />,
                                                            destructive: true,
                                                            disabled: busy,
                                                            onClick: () => {
                                                                void handleDeleteSession(s.session_id);
                                                            },
                                                        }]}
                                                        primary={{
                                                            key: "open",
                                                            label: "Open",
                                                            icon: <Open20Regular />,
                                                            onClick: () => {
                                                                if (s.interaction_mode === "cli") {
                                                                    onNavigateToCli(s.session_id);
                                                                } else {
                                                                    onNavigateToChat(s.session_id);
                                                                }
                                                            },
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
        </div>
    );
}
