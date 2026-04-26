import type { FormEvent } from "react";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { Connection } from "./types";
import { api } from "./api";
import { queryKeys, useConnections } from "./queries";
import { useErrorContext } from "./ErrorContext";

export default function ConnectionsView() {
    const { data: connections = [] } = useConnections();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [showForm, setShowForm] = useState(false);
    const [formId, setFormId] = useState("");
    const [formName, setFormName] = useState("");
    const [formUrl, setFormUrl] = useState("");
    const [formApiKey, setFormApiKey] = useState("");

    const [editingId, setEditingId] = useState<string | null>(null);
    const [editName, setEditName] = useState("");
    const [editUrl, setEditUrl] = useState("");
    const [editApiKey, setEditApiKey] = useState("");

    const invalidate = () =>
        queryClient.invalidateQueries({ queryKey: queryKeys.connections });

    const createMutation = useMutation({
        mutationFn: (payload: {
            connection_id: string;
            name: string;
            url: string;
            api_key: string;
        }) => api.createConnection(payload),
        onSuccess: () => invalidate(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({ id, payload }: { id: string; payload: { name?: string; url?: string; api_key?: string } }) =>
            api.updateConnection(id, payload),
        onSuccess: () => invalidate(),
        onError: reportError,
    });

    const deleteMutation = useMutation({
        mutationFn: (id: string) => api.deleteConnection(id),
        onSuccess: () => invalidate(),
        onError: reportError,
    });

    const busy =
        createMutation.isPending
        || updateMutation.isPending
        || deleteMutation.isPending;

    async function handleSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        await createMutation.mutateAsync({
            connection_id: formId,
            name: formName,
            url: formUrl,
            api_key: formApiKey,
        });
        setFormId("");
        setFormName("");
        setFormUrl("");
        setFormApiKey("");
        setShowForm(false);
    }

    function openEdit(conn: Connection) {
        setEditingId(conn.connection_id);
        setEditName(conn.name);
        setEditUrl(conn.url);
        setEditApiKey("");
    }

    function cancelEdit() {
        setEditingId(null);
        setEditName("");
        setEditUrl("");
        setEditApiKey("");
    }

    async function handleEditSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        if (!editingId) return;
        await updateMutation.mutateAsync({
            id: editingId,
            payload: {
                name: editName,
                url: editUrl,
                api_key: editApiKey || undefined,
            },
        });
        cancelEdit();
    }

    const editingConn = editingId
        ? (connections.find((c) => c.connection_id === editingId) ?? null)
        : null;

    return (
        <div className="view-content">
            <div className="view-header">
                <h2>Connections</h2>
                <button onClick={() => setShowForm(!showForm)} type="button">
                    {showForm ? "Cancel" : "New Connection"}
                </button>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={(e) => { void handleSubmit(e); }}>
                    <label>
                        Connection ID
                        <input
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="openai"
                            required
                            value={formId}
                            onChange={(e) => setFormId(e.target.value)}
                        />
                    </label>
                    <label>
                        Display Name
                        <input
                            placeholder="OpenAI"
                            required
                            value={formName}
                            onChange={(e) => setFormName(e.target.value)}
                        />
                    </label>
                    <label>
                        URL
                        <input
                            placeholder="https://api.openai.com/v1"
                            required
                            value={formUrl}
                            onChange={(e) => setFormUrl(e.target.value)}
                        />
                    </label>
                    <label>
                        API Key
                        <input
                            placeholder="sk-..."
                            type="password"
                            value={formApiKey}
                            onChange={(e) => setFormApiKey(e.target.value)}
                        />
                        <span className="muted">Leave blank if the endpoint does not require a key</span>
                    </label>
                    <button disabled={busy} type="submit">
                        Create Connection
                    </button>
                </form>
            )}

            <div className="card-grid">
                {connections.map((conn) => (
                    <div className="card" key={conn.connection_id}>
                        <div className="card-body">
                            <h3>{conn.name}</h3>
                            <div className="muted">{conn.connection_id}</div>
                            <div className="card-meta">
                                <div>
                                    <strong>URL:</strong> {conn.url}
                                </div>
                                <div>
                                    <strong>API Key:</strong>{" "}
                                    {conn.has_api_key ? "set" : "not set"}
                                </div>
                            </div>
                            {editingId === conn.connection_id && (
                                <form
                                    className="create-form"
                                    onSubmit={(e) => { void handleEditSubmit(e); }}
                                >
                                    <label>
                                        Display Name
                                        <input
                                            required
                                            value={editName}
                                            onChange={(e) => setEditName(e.target.value)}
                                        />
                                    </label>
                                    <label>
                                        URL
                                        <input
                                            required
                                            value={editUrl}
                                            onChange={(e) => setEditUrl(e.target.value)}
                                        />
                                    </label>
                                    <label>
                                        API Key
                                        <input
                                            placeholder={editingConn && editingConn.has_api_key ? "(leave blank to keep current value)" : "sk-..."}
                                            type="password"
                                            value={editApiKey}
                                            onChange={(e) => setEditApiKey(e.target.value)}
                                        />
                                    </label>
                                    <div className="card-footer-actions">
                                        <button
                                            disabled={busy}
                                            type="submit"
                                        >
                                            Save Changes
                                        </button>
                                        <button
                                            className="secondary-button"
                                            onClick={cancelEdit}
                                            type="button"
                                        >
                                            Cancel
                                        </button>
                                    </div>
                                </form>
                            )}
                        </div>
                        <div className="card-footer">
                            <span className="muted">
                                Created {new Date(conn.created_at).toLocaleDateString()}
                            </span>
                            <div className="card-footer-actions">
                                {editingId !== conn.connection_id && (
                                    <button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => openEdit(conn)}
                                        type="button"
                                    >
                                        Edit
                                    </button>
                                )}
                                <button
                                    className="danger-button small"
                                    disabled={busy}
                                    onClick={() => deleteMutation.mutate(conn.connection_id)}
                                    type="button"
                                >
                                    Delete
                                </button>
                            </div>
                        </div>
                    </div>
                ))}
                {connections.length === 0 && (
                    <div className="empty-state">
                        No connections yet. Create one to add an LLM endpoint for your agents.
                    </div>
                )}
            </div>
        </div>
    );
}
