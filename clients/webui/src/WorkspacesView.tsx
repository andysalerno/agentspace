import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
    Add20Regular,
    Copy20Regular,
    Delete20Regular,
    Edit20Regular,
    Folder24Regular,
    Open20Regular,
} from "@fluentui/react-icons";
import { api } from "./api";
import { browserReachableLocalUrl } from "./browserUrls";
import { queryKeys, useAgents, useWorkspaces } from "./queries";
import { useErrorContext } from "./useErrorContext";
import { WORKSPACE_ID_PATTERN, workspaceIdFromName } from "./saveWorkspacePrompt";
import type { Workspace, WorkspaceVscode } from "./types";
import { Button, Field, Input } from "./fluent";
import { EmptyState, FormDialog, RowActions, StatusBadge, ViewHeader } from "./ui";
import { statusTone } from "./status";

export default function WorkspacesView() {
    const { data: workspaces = [] } = useWorkspaces();
    const { data: agents = [] } = useAgents();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [showForm, setShowForm] = useState(false);
    const [workspaceId, setWorkspaceId] = useState("");
    const [name, setName] = useState("");
    const [editing, setEditing] = useState<Workspace | null>(null);
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
            { id, workspace_id, name: cloneName }: {
                id: string;
                workspace_id: string;
                name: string;
            },
        ) => api.cloneWorkspace(id, { workspace_id, name: cloneName }),
        onSuccess: () => invalidateWorkspaces(),
        onError: reportError,
    });
    const vscodeMutation = useMutation({
        mutationFn: (id: string) => api.openWorkspaceVscode(id),
        onError: reportError,
    });

    const busy = createMutation.isPending
        || updateMutation.isPending
        || deleteMutation.isPending
        || cloneMutation.isPending
        || vscodeMutation.isPending;

    function mountedAgentCount(targetWorkspaceId: string): number {
        return agents.filter((agent) =>
            agent.workspace_mounts.some((mount) => mount.workspace_id === targetWorkspaceId)
        ).length;
    }

    async function handleCreate() {
        await createMutation.mutateAsync({ workspace_id: workspaceId, name });
        setWorkspaceId("");
        setName("");
        setShowForm(false);
    }

    async function handleSaveEdit() {
        if (editing === null) return;
        await updateMutation.mutateAsync({ id: editing.workspace_id, nextName: editName });
        setEditing(null);
    }

    async function handleClone(workspace: Workspace) {
        const cloneName = window.prompt("Clone workspace name", `${workspace.name} Copy`);
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
        <div className="view-content">
            <ViewHeader
                actions={
                    <Button
                        appearance="primary"
                        icon={<Add20Regular />}
                        onClick={() => setShowForm(true)}
                        type="button"
                    >
                        New workspace
                    </Button>
                }
                description="Docker volumes mounted into agent kernels."
                title="Workspaces"
            />
            <div className="view-body">
                {workspaces.length === 0
                    ? (
                        <EmptyState
                            action={
                                <Button appearance="primary" onClick={() => setShowForm(true)}>
                                    New workspace
                                </Button>
                            }
                            description="Workspaces give agents persistent storage that survives kernel restarts."
                            icon={<Folder24Regular />}
                            title="No workspaces yet"
                        />
                    )
                    : (
                        <div className="table-container">
                            <div className="table-scroll">
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th>Workspace</th>
                                            <th>Status</th>
                                            <th>Mount path</th>
                                            <th>Volume</th>
                                            <th className="num">Agents</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {workspaces.map((workspace) => {
                                            const mountedCount = mountedAgentCount(
                                                workspace.workspace_id,
                                            );
                                            const isBuiltin = workspace.builtin === true;
                                            const ready = workspace.status === "ready";
                                            return (
                                                <tr key={workspace.workspace_id}>
                                                    <td>
                                                        <div className="cell-identity">
                                                            <span className="cell-identity-name">
                                                                {workspace.name}
                                                                {isBuiltin && (
                                                                    <span className="tag" style={{ marginLeft: 8 }}>
                                                                        Built-in
                                                                    </span>
                                                                )}
                                                            </span>
                                                            <span className="cell-identity-id">
                                                                {workspace.workspace_id}
                                                            </span>
                                                        </div>
                                                    </td>
                                                    <td>
                                                        <StatusBadge
                                                            label={workspace.status}
                                                            tone={statusTone(workspace.status)}
                                                        />
                                                    </td>
                                                    <td className="mono-sm">{workspace.mount_path}</td>
                                                    <td
                                                        className="mono-sm muted"
                                                        title={workspace.volume_name}
                                                    >
                                                        <span className="truncate">
                                                            {workspace.volume_name}
                                                        </span>
                                                    </td>
                                                    <td className="num">{mountedCount}</td>
                                                    <td className="actions-cell">
                                                        <RowActions
                                                            items={isBuiltin ? [] : [
                                                                {
                                                                    key: "rename",
                                                                    label: "Rename",
                                                                    icon: <Edit20Regular />,
                                                                    disabled: busy,
                                                                    onClick: () => {
                                                                        setEditing(workspace);
                                                                        setEditName(workspace.name);
                                                                    },
                                                                },
                                                                {
                                                                    key: "clone",
                                                                    label: "Clone",
                                                                    icon: <Copy20Regular />,
                                                                    disabled: busy || !ready,
                                                                    onClick: () => {
                                                                        void handleClone(workspace);
                                                                    },
                                                                },
                                                                {
                                                                    key: "delete",
                                                                    label: mountedCount > 0
                                                                        ? "Delete (in use)"
                                                                        : "Delete",
                                                                    icon: <Delete20Regular />,
                                                                    destructive: true,
                                                                    disabled: busy || mountedCount > 0,
                                                                    confirm:
                                                                        `Delete "${workspace.name}"? Its volume and contents are destroyed.`,
                                                                    onClick: () =>
                                                                        deleteMutation.mutate(
                                                                            workspace.workspace_id,
                                                                        ),
                                                                },
                                                            ]}
                                                            primary={{
                                                                key: "vscode",
                                                                label: "Open in VS Code",
                                                                icon: <Open20Regular />,
                                                                disabled: busy || !ready,
                                                                onClick: () => {
                                                                    void handleOpenVscode(workspace);
                                                                },
                                                            }}
                                                        />
                                                    </td>
                                                </tr>
                                            );
                                        })}
                                    </tbody>
                                </table>
                            </div>
                        </div>
                    )}
            </div>

            <FormDialog
                busy={busy}
                onOpenChange={setShowForm}
                onSubmit={() => {
                    void handleCreate();
                }}
                open={showForm}
                submitLabel="Create workspace"
                title="New workspace"
            >
                <div className="form-grid">
                    <Field
                        hint="Used in the mount path: /workspace/<id>"
                        label="Workspace ID"
                        required
                    >
                        <Input
                            onChange={(e) => setWorkspaceId(e.target.value)}
                            pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
                            placeholder="todo-list-code"
                            required
                            value={workspaceId}
                        />
                    </Field>
                    <Field label="Display name" required>
                        <Input
                            onChange={(e) => setName(e.target.value)}
                            placeholder="Todo list code"
                            required
                            value={name}
                        />
                    </Field>
                </div>
            </FormDialog>

            <FormDialog
                busy={busy}
                onOpenChange={(open) => {
                    if (!open) setEditing(null);
                }}
                onSubmit={() => {
                    void handleSaveEdit();
                }}
                open={editing !== null}
                submitLabel="Save"
                title={`Rename ${editing?.name ?? "workspace"}`}
            >
                <Field label="Display name" required>
                    <Input
                        onChange={(e) => setEditName(e.target.value)}
                        required
                        value={editName}
                    />
                </Field>
            </FormDialog>
        </div>
    );
}
