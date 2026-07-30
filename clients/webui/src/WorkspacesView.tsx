import type { FormEvent } from "react";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { browserReachableLocalUrl } from "./browserUrls";
import { queryKeys, useAgents, useWorkspaces } from "./queries";
import { useErrorContext } from "./useErrorContext";
import { WORKSPACE_ID_PATTERN, workspaceIdFromName } from "./saveWorkspacePrompt";
import type { Workspace, WorkspaceVscode } from "./types";
import { Button, Input } from "./fluent";

export default function WorkspacesView() {
    const { data: workspaces = [] } = useWorkspaces();
    const { data: agents = [] } = useAgents();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [showForm, setShowForm] = useState(false);
    const [workspaceId, setWorkspaceId] = useState("");
    const [name, setName] = useState("");
    const [editingWorkspaceId, setEditingWorkspaceId] = useState<string | null>(null);
    const [editName, setEditName] = useState("");

    const invalidateWorkspaces = () =>
        queryClient.invalidateQueries({ queryKey: queryKeys.workspaces });

    const createMutation = useMutation({
        mutationFn: (payload: { workspace_id: string; name: string }) =>
            api.createWorkspace(payload),
        onSuccess: () => invalidateWorkspaces(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({ id, nextName }: { id: string; nextName: string }) =>
            api.updateWorkspace(id, { name: nextName }),
        onSuccess: () => invalidateWorkspaces(),
        onError: reportError,
    });

    const deleteMutation = useMutation({
        mutationFn: (id: string) => api.deleteWorkspace(id),
        onSuccess: () => invalidateWorkspaces(),
        onError: reportError,
    });
    const cloneMutation = useMutation({
        mutationFn: (
            { id, workspace_id, name }: { id: string; workspace_id: string; name: string },
        ) =>
            api.cloneWorkspace(id, { workspace_id, name }),
        onSuccess: () => invalidateWorkspaces(),
        onError: reportError,
    });
    const vscodeMutation = useMutation({
        mutationFn: (id: string) => api.openWorkspaceVscode(id),
        onError: reportError,
    });

    const busy =
        createMutation.isPending
        || updateMutation.isPending
        || deleteMutation.isPending
        || cloneMutation.isPending
        || vscodeMutation.isPending;

    function mountedAgentCount(targetWorkspaceId: string): number {
        return agents.filter((agent) =>
            agent.workspace_mounts.some(
                (mount) => mount.workspace_id === targetWorkspaceId,
            ),
        ).length;
    }

    async function handleSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        await createMutation.mutateAsync({
            workspace_id: workspaceId,
            name,
        });
        setWorkspaceId("");
        setName("");
        setShowForm(false);
    }

    function startEditing(id: string, currentName: string) {
        setEditingWorkspaceId(id);
        setEditName(currentName);
    }

    async function saveEdit(id: string) {
        await updateMutation.mutateAsync({ id, nextName: editName });
        setEditingWorkspaceId(null);
        setEditName("");
    }

    async function handleClone(workspace: Workspace) {
        const cloneName = window.prompt(
            "Clone workspace name",
            `${workspace.name} Copy`,
        );
        if (cloneName === null) return;
        const trimmedName = cloneName.trim();
        if (!trimmedName) {
            window.alert("Workspace name is required.");
            return;
        }
        const cloneId = window.prompt(
            "Clone workspace ID (lowercase letters, numbers, and single dashes)",
            workspaceIdFromName(trimmedName),
        );
        if (cloneId === null) return;
        const trimmedId = cloneId.trim();
        if (!WORKSPACE_ID_PATTERN.test(trimmedId)) {
            window.alert("Workspace ID must use lowercase letters, numbers, and single dashes.");
            return;
        }
        await cloneMutation.mutateAsync({
            id: workspace.workspace_id,
            workspace_id: trimmedId,
            name: trimmedName,
        });
    }

    async function handleOpenVscode(workspace: Workspace) {
        const editorWindow = window.open("about:blank", "_blank");
        let result: WorkspaceVscode;
        try {
            result = await vscodeMutation.mutateAsync(workspace.workspace_id);
        } catch {
            editorWindow?.close();
            return;
        }
        if (result.vscode_url) {
            const vscodeUrl = browserReachableLocalUrl(result.vscode_url);
            if (editorWindow) {
                editorWindow.location.href = vscodeUrl;
            } else {
                window.open(vscodeUrl, "_blank", "noopener,noreferrer");
            }
        } else {
            editorWindow?.close();
            window.alert("VS Code is unavailable for this workspace.");
        }
    }

    return (
        <div className="view-content management-view workspaces-management-view">
            <div className="view-header">
                <div>
                    <h2>Workspaces</h2>
                    <span className="muted">
                        {workspaces.length} configured · Docker volumes mounted into agent kernels
                    </span>
                </div>
                <div className="view-header-actions">
                    <Button onClick={() => setShowForm(!showForm)} type="button">
                        {showForm ? "Cancel" : "New Workspace"}
                    </Button>
                </div>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={(e) => { void handleSubmit(e); }}>
                    <label>
                        Workspace ID
                        <Input
                            pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
                            placeholder="todo-list-code"
                            required
                            value={workspaceId}
                            onChange={(e) => setWorkspaceId(e.target.value)}
                        />
                        <span className="muted">Used in the mount path: /workspace/&lt;id&gt;</span>
                    </label>
                    <label>
                        Display Name
                        <Input
                            placeholder="TodoListCode"
                            required
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                        />
                    </label>
                    <Button disabled={busy} type="submit">
                        Create Workspace
                    </Button>
                </form>
            )}

            <div className="card-grid management-card-grid">
                {workspaces.map((workspace) => {
                    const mountedCount = mountedAgentCount(workspace.workspace_id);
                    const editing = editingWorkspaceId === workspace.workspace_id;
                    const isBuiltin = workspace.builtin === true;
                    return (
                        <div className="card management-card" key={workspace.workspace_id}>
                            <div className="card-body">
                                <div className="management-card-heading">
                                    <div className="management-title-block">
                                        <h3>{workspace.name}</h3>
                                        <code className="management-id">{workspace.workspace_id}</code>
                                    </div>
                                    <div className="tag-row">
                                        {isBuiltin && <span className="tag">Built-in</span>}
                                        <span className="tag">{mountedCount} agent{mountedCount === 1 ? "" : "s"}</span>
                                    </div>
                                </div>
                                <div className="card-meta management-meta">
                                    <div>
                                        <strong>Status</strong>
                                        <span className={`status-badge ${workspace.status}`}>{workspace.status}</span>
                                    </div>
                                    <div>
                                        <strong>Mount Path</strong>
                                        <span className="mono">{workspace.mount_path}</span>
                                    </div>
                                    <div>
                                        <strong>Volume</strong>
                                        <span className="mono truncate-value">{workspace.volume_name}</span>
                                    </div>
                                </div>
                                {editing && (
                                    <form
                                        className="create-form"
                                        onSubmit={(e) => {
                                            e.preventDefault();
                                            void saveEdit(workspace.workspace_id);
                                        }}
                                    >
                                        <label>
                                            Display Name
                                            <Input
                                                required
                                                value={editName}
                                                onChange={(e) => setEditName(e.target.value)}
                                            />
                                        </label>
                                        <div className="skills-edit-actions">
                                            <Button className="small" disabled={busy} type="submit">
                                                Save
                                            </Button>
                                            <Button
                                                className="secondary-button small"
                                                onClick={() => setEditingWorkspaceId(null)}
                                                type="button"
                                            >
                                                Cancel
                                            </Button>
                                        </div>
                                    </form>
                                )}
                            </div>
                            <div className="card-footer">
                                <span className="muted">
                                    {isBuiltin
                                        ? "Built-in workspace"
                                        : `Created ${new Date(workspace.created_at).toLocaleDateString()}`}
                                </span>
                                <div className="card-footer-actions">
                                    {!editing && (
                                        <>
                                            <Button
                                                className="secondary-button small"
                                                disabled={busy || workspace.status !== "ready"}
                                                onClick={() => void handleOpenVscode(workspace)}
                                                type="button"
                                            >
                                                Open in VS Code
                                            </Button>
                                            {!isBuiltin && (
                                                <>
                                                    <Button
                                                        className="secondary-button small"
                                                        disabled={busy || workspace.status !== "ready"}
                                                        onClick={() => void handleClone(workspace)}
                                                        type="button"
                                                    >
                                                        Clone
                                                    </Button>
                                                    <Button
                                                        className="secondary-button small"
                                                        disabled={busy}
                                                        onClick={() => startEditing(workspace.workspace_id, workspace.name)}
                                                        type="button"
                                                    >
                                                        Edit
                                                    </Button>
                                                </>
                                            )}
                                        </>
                                    )}
                                    {!isBuiltin && (
                                        <Button
                                            className="danger-button small"
                                            disabled={busy || mountedCount > 0}
                                            onClick={() => deleteMutation.mutate(workspace.workspace_id)}
                                            title={mountedCount > 0 ? "Remove this workspace from agents before deleting it." : undefined}
                                            type="button"
                                        >
                                            Delete
                                        </Button>
                                    )}
                                </div>
                            </div>
                        </div>
                    );
                })}
                {workspaces.length === 0 && (
                    <div className="empty-state">No workspaces yet. Create one to give agents persistent shared storage.</div>
                )}
            </div>
        </div>
    );
}
