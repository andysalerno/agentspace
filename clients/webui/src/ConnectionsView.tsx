import type { FormEvent } from "react";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { Connection } from "./types";
import { api } from "./api";
import { queryKeys, useConnections } from "./queries";
import { useErrorContext } from "./ErrorContext";
import { Button, Input, Select } from "./fluent";

type ConnectionApiFlavor = Connection["api_flavor"];

const API_FLAVOR_OPTIONS: Array<{ value: ConnectionApiFlavor; label: string }> = [
    { value: "chat_completions", label: "Chat completions" },
    { value: "responses", label: "Responses" },
];

const apiFlavorLabel = (value: ConnectionApiFlavor) =>
    API_FLAVOR_OPTIONS.find((option) => option.value === value)?.label ?? value;

export default function ConnectionsView() {
    const { data: connections = [] } = useConnections();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [showForm, setShowForm] = useState(false);
    const [formId, setFormId] = useState("");
    const [formName, setFormName] = useState("");
    const [formUrl, setFormUrl] = useState("");
    const [formApiFlavor, setFormApiFlavor] = useState<ConnectionApiFlavor>("chat_completions");
    const [formApiKey, setFormApiKey] = useState("");

    const [editingId, setEditingId] = useState<string | null>(null);
    const [editName, setEditName] = useState("");
    const [editUrl, setEditUrl] = useState("");
    const [editApiFlavor, setEditApiFlavor] = useState<ConnectionApiFlavor>("chat_completions");
    const [editApiKey, setEditApiKey] = useState("");

    const invalidate = () =>
        queryClient.invalidateQueries({ queryKey: queryKeys.connections });

    const createMutation = useMutation({
        mutationFn: (payload: {
            connection_id: string;
            name: string;
            url: string;
            api_flavor: ConnectionApiFlavor;
            api_key: string;
        }) => api.createConnection(payload),
        onSuccess: () => invalidate(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({ id, payload }: { id: string; payload: { name?: string; url?: string; api_flavor?: ConnectionApiFlavor; api_key?: string } }) =>
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
            api_flavor: formApiFlavor,
            api_key: formApiKey,
        });
        setFormId("");
        setFormName("");
        setFormUrl("");
        setFormApiFlavor("chat_completions");
        setFormApiKey("");
        setShowForm(false);
    }

    function openEdit(conn: Connection) {
        setEditingId(conn.connection_id);
        setEditName(conn.name);
        setEditUrl(conn.url);
        setEditApiFlavor(conn.api_flavor);
        setEditApiKey("");
    }

    function cancelEdit() {
        setEditingId(null);
        setEditName("");
        setEditUrl("");
        setEditApiFlavor("chat_completions");
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
                api_flavor: editApiFlavor,
                api_key: editApiKey || undefined,
            },
        });
        cancelEdit();
    }

    const editingConn = editingId
        ? (connections.find((c) => c.connection_id === editingId) ?? null)
        : null;

    return (
        <div className="view-content management-view connections-management-view">
            <div className="view-header">
                <div>
                    <h2>Connections</h2>
                    <span className="muted">
                        {connections.length} endpoints · {connections.filter((conn) => conn.has_api_key).length} keyed
                    </span>
                </div>
                <div className="view-header-actions">
                    <Button onClick={() => setShowForm(!showForm)} type="button">
                        {showForm ? "Cancel" : "New Connection"}
                    </Button>
                </div>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={(e) => { void handleSubmit(e); }}>
                    <label>
                        Connection ID
                        <Input
                            autoComplete="username"
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="openai"
                            required
                            value={formId}
                            onChange={(e) => setFormId(e.target.value)}
                        />
                    </label>
                    <label>
                        Display Name
                        <Input
                            autoComplete="organization"
                            placeholder="OpenAI"
                            required
                            value={formName}
                            onChange={(e) => setFormName(e.target.value)}
                        />
                    </label>
                    <label>
                        URL
                        <Input
                            autoComplete="url"
                            placeholder="https://api.openai.com/v1"
                            required
                            value={formUrl}
                            onChange={(e) => setFormUrl(e.target.value)}
                        />
                    </label>
                    <label>
                        API Key
                        <Input
                            autoComplete="new-password"
                            placeholder="sk-..."
                            type="password"
                            value={formApiKey}
                            onChange={(e) => setFormApiKey(e.target.value)}
                        />
                        <span className="muted">Leave blank if the endpoint does not require a key</span>
                    </label>
                    <label>
                        API Flavor
                        <Select
                            required
                            value={formApiFlavor}
                            onChange={(e) => setFormApiFlavor(e.target.value as ConnectionApiFlavor)}
                        >
                            {API_FLAVOR_OPTIONS.map((option) => (
                                <option key={option.value} value={option.value}>
                                    {option.label}
                                </option>
                            ))}
                        </Select>
                    </label>
                    <Button disabled={busy} type="submit">
                        Create Connection
                    </Button>
                </form>
            )}

            <div className="card-grid management-card-grid">
                {connections.map((conn) => (
                    <div className="card management-card" key={conn.connection_id}>
                        <div className="card-body">
                            <div className="management-card-heading">
                                <div className="management-title-block">
                                    <h3>{conn.name}</h3>
                                    <code className="management-id">{conn.connection_id}</code>
                                </div>
                                <span className={`status-badge ${conn.has_api_key ? "active" : "stopped"}`}>
                                    {conn.has_api_key ? "key set" : "no key"}
                                </span>
                            </div>
                            <div className="card-meta">
                                <div>
                                    <strong>URL:</strong>{" "}
                                    <span className="truncate-value" title={conn.url}>{conn.url}</span>
                                </div>
                                <div>
                                    <strong>API Key:</strong>{" "}
                                    {conn.has_api_key ? "set" : "not set"}
                                </div>
                                <div>
                                    <strong>API Flavor:</strong> {apiFlavorLabel(conn.api_flavor)}
                                </div>
                            </div>
                            {editingId === conn.connection_id && (
                                <form
                                    className="create-form"
                                    onSubmit={(e) => { void handleEditSubmit(e); }}
                                >
                                    <label>
                                        Display Name
                                        <Input
                                            autoComplete="organization"
                                            required
                                            value={editName}
                                            onChange={(e) => setEditName(e.target.value)}
                                        />
                                    </label>
                                    <label>
                                        URL
                                        <Input
                                            autoComplete="url"
                                            required
                                            value={editUrl}
                                            onChange={(e) => setEditUrl(e.target.value)}
                                        />
                                    </label>
                                    <label>
                                        API Key
                                        <Input
                                            autoComplete="new-password"
                                            placeholder={editingConn && editingConn.has_api_key ? "(leave blank to keep current value)" : "sk-..."}
                                            type="password"
                                            value={editApiKey}
                                            onChange={(e) => setEditApiKey(e.target.value)}
                                        />
                                    </label>
                                    <label>
                                        API Flavor
                                        <Select
                                            required
                                            value={editApiFlavor}
                                            onChange={(e) => setEditApiFlavor(e.target.value as ConnectionApiFlavor)}
                                        >
                                            {API_FLAVOR_OPTIONS.map((option) => (
                                                <option key={option.value} value={option.value}>
                                                    {option.label}
                                                </option>
                                            ))}
                                        </Select>
                                    </label>
                                    <div className="card-footer-actions">
                                        <Button
                                            disabled={busy}
                                            type="submit"
                                        >
                                            Save Changes
                                        </Button>
                                        <Button
                                            className="secondary-button"
                                            onClick={cancelEdit}
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
                                Created {new Date(conn.created_at).toLocaleDateString()}
                            </span>
                            <div className="card-footer-actions">
                                <Button
                                    className="secondary-button small"
                                    onClick={() => {
                                        void api.downloadConfigResource(
                                            "connection",
                                            conn.connection_id,
                                        ).catch(reportError);
                                    }}
                                    type="button"
                                >
                                    Export YAML
                                </Button>
                                {editingId !== conn.connection_id && (
                                    <Button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => openEdit(conn)}
                                        type="button"
                                    >
                                        Edit
                                    </Button>
                                )}
                                <Button
                                    className="danger-button small"
                                    disabled={busy}
                                    onClick={() => deleteMutation.mutate(conn.connection_id)}
                                    type="button"
                                >
                                    Delete
                                </Button>
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
