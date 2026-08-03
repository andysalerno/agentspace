import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
    Add20Regular,
    ArrowDownload20Regular,
    Delete20Regular,
    Edit20Regular,
    PlugConnected24Regular,
} from "@fluentui/react-icons";
import type { Connection } from "./types";
import { api } from "./api";
import { queryKeys, useConnections } from "./queries";
import { useErrorContext } from "./useErrorContext";
import { Button, Field, Input, Select } from "./fluent";
import SecretRefSelect, { LITERAL_VALUE } from "./SecretRefSelect";
import { EmptyState, FormDialog, RowActions, StatusBadge, ViewHeader } from "./ui";

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
    const [formApiKeySecret, setFormApiKeySecret] = useState("");

    const [editingId, setEditingId] = useState<string | null>(null);
    const [editName, setEditName] = useState("");
    const [editUrl, setEditUrl] = useState("");
    const [editApiFlavor, setEditApiFlavor] = useState<ConnectionApiFlavor>("chat_completions");
    const [editApiKeySecret, setEditApiKeySecret] = useState("");

    const invalidate = () => queryClient.invalidateQueries({ queryKey: queryKeys.connections });

    const createMutation = useMutation({
        mutationFn: (payload: {
            connection_id: string;
            name: string;
            url: string;
            api_flavor: ConnectionApiFlavor;
            api_key_secret: string;
        }) => api.createConnection(payload),
        onSuccess: () => invalidate(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({ id, payload }: {
            id: string;
            payload: {
                name?: string;
                url?: string;
                api_flavor?: ConnectionApiFlavor;
                api_key_secret?: string;
            };
        }) => api.updateConnection(id, payload),
        onSuccess: () => invalidate(),
        onError: reportError,
    });

    const deleteMutation = useMutation({
        mutationFn: (id: string) => api.deleteConnection(id),
        onSuccess: () => invalidate(),
        onError: reportError,
    });

    const busy = createMutation.isPending || updateMutation.isPending || deleteMutation.isPending;

    async function handleCreate() {
        await createMutation.mutateAsync({
            connection_id: formId,
            name: formName,
            url: formUrl,
            api_flavor: formApiFlavor,
            api_key_secret: formApiKeySecret,
        });
        setFormId("");
        setFormName("");
        setFormUrl("");
        setFormApiFlavor("chat_completions");
        setFormApiKeySecret("");
        setShowForm(false);
    }

    function openEdit(conn: Connection) {
        setEditingId(conn.connection_id);
        setEditName(conn.name);
        setEditUrl(conn.url);
        setEditApiFlavor(conn.api_flavor);
        // A literal key is only authorable in YAML, so it is represented by a
        // sentinel the picker preserves rather than silently clearing.
        setEditApiKeySecret(conn.api_key_secret ?? (conn.has_api_key ? LITERAL_VALUE : ""));
    }

    async function handleEditSubmit() {
        if (editingId === null) return;
        await updateMutation.mutateAsync({
            id: editingId,
            payload: {
                name: editName,
                url: editUrl,
                api_flavor: editApiFlavor,
                // Omitted while the YAML literal is still selected, so editing
                // an unrelated field cannot clear an authored key.
                ...(editApiKeySecret === LITERAL_VALUE ? {} : { api_key_secret: editApiKeySecret }),
            },
        });
        setEditingId(null);
    }

    const keyedCount = connections.filter((conn) => conn.has_api_key).length;

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
                        New connection
                    </Button>
                }
                description={`${connections.length} endpoints, ${keyedCount} with a key`}
                title="Connections"
            />
            <div className="view-body">
                {connections.length === 0
                    ? (
                        <EmptyState
                            action={
                                <Button appearance="primary" onClick={() => setShowForm(true)}>
                                    New connection
                                </Button>
                            }
                            description="A connection points agents at a model endpoint and the secret that authenticates it."
                            icon={<PlugConnected24Regular />}
                            title="No connections yet"
                        />
                    )
                    : (
                        <div className="table-container">
                            <div className="table-scroll">
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th>Connection</th>
                                            <th>Endpoint</th>
                                            <th>API</th>
                                            <th>Key</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {connections.map((conn) => (
                                            <tr key={conn.connection_id}>
                                                <td>
                                                    <div className="cell-identity">
                                                        <span className="cell-identity-name">
                                                            {conn.name}
                                                        </span>
                                                        <span className="cell-identity-id">
                                                            {conn.connection_id}
                                                        </span>
                                                    </div>
                                                </td>
                                                <td className="cell-wrap">
                                                    <span className="mono-sm" title={conn.url}>
                                                        {conn.url}
                                                    </span>
                                                </td>
                                                <td className="muted">
                                                    {apiFlavorLabel(conn.api_flavor)}
                                                </td>
                                                <td>
                                                    {conn.has_api_key
                                                        ? (
                                                            <StatusBadge
                                                                label={conn.api_key_secret
                                                                    ?? "Literal in YAML"}
                                                                tone="ok"
                                                            />
                                                        )
                                                        : (
                                                            <StatusBadge
                                                                label="Not set"
                                                                tone="neutral"
                                                            />
                                                        )}
                                                </td>
                                                <td className="actions-cell">
                                                    <RowActions
                                                        items={[
                                                            {
                                                                key: "export",
                                                                label: "Export YAML",
                                                                icon: <ArrowDownload20Regular />,
                                                                onClick: () => {
                                                                    void api.downloadConfigResource(
                                                                        "connection",
                                                                        conn.connection_id,
                                                                    ).catch(reportError);
                                                                },
                                                            },
                                                            {
                                                                key: "delete",
                                                                label: "Delete connection",
                                                                icon: <Delete20Regular />,
                                                                destructive: true,
                                                                disabled: busy,
                                                                confirm:
                                                                    `Delete the connection "${conn.name}"? Agents using it stop working.`,
                                                                onClick: () =>
                                                                    deleteMutation.mutate(
                                                                        conn.connection_id,
                                                                    ),
                                                            },
                                                        ]}
                                                        primary={{
                                                            key: "edit",
                                                            label: "Edit",
                                                            icon: <Edit20Regular />,
                                                            disabled: busy,
                                                            onClick: () => openEdit(conn),
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

            <FormDialog
                busy={busy}
                onOpenChange={setShowForm}
                onSubmit={() => {
                    void handleCreate();
                }}
                open={showForm}
                submitLabel="Create connection"
                title="New connection"
            >
                <div className="form-grid">
                    <Field label="Connection ID" required>
                        <Input
                            autoComplete="username"
                            onChange={(e) => setFormId(e.target.value)}
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="openai"
                            required
                            value={formId}
                        />
                    </Field>
                    <Field label="Display name" required>
                        <Input
                            autoComplete="organization"
                            onChange={(e) => setFormName(e.target.value)}
                            placeholder="OpenAI"
                            required
                            value={formName}
                        />
                    </Field>
                    <div className="span-2">
                        <Field label="Endpoint URL" required>
                            <Input
                                autoComplete="url"
                                onChange={(e) => setFormUrl(e.target.value)}
                                placeholder="https://api.openai.com/v1"
                                required
                                value={formUrl}
                            />
                        </Field>
                    </div>
                    <Field label="API flavor" required>
                        <Select
                            onChange={(e) =>
                                setFormApiFlavor(e.target.value as ConnectionApiFlavor)}
                            required
                            value={formApiFlavor}
                        >
                            {API_FLAVOR_OPTIONS.map((option) => (
                                <option key={option.value} value={option.value}>
                                    {option.label}
                                </option>
                            ))}
                        </Select>
                    </Field>
                    <SecretRefSelect
                        label="API key secret"
                        noneLabel="No API key"
                        onChange={setFormApiKeySecret}
                        value={formApiKeySecret}
                    />
                </div>
            </FormDialog>

            <FormDialog
                busy={busy}
                onOpenChange={(open) => {
                    if (!open) setEditingId(null);
                }}
                onSubmit={() => {
                    void handleEditSubmit();
                }}
                open={editingId !== null}
                submitLabel="Save changes"
                title={`Edit ${editingId ?? ""}`}
            >
                <div className="form-grid">
                    <Field label="Display name" required>
                        <Input
                            autoComplete="organization"
                            onChange={(e) => setEditName(e.target.value)}
                            required
                            value={editName}
                        />
                    </Field>
                    <Field label="API flavor" required>
                        <Select
                            onChange={(e) =>
                                setEditApiFlavor(e.target.value as ConnectionApiFlavor)}
                            required
                            value={editApiFlavor}
                        >
                            {API_FLAVOR_OPTIONS.map((option) => (
                                <option key={option.value} value={option.value}>
                                    {option.label}
                                </option>
                            ))}
                        </Select>
                    </Field>
                    <div className="span-2">
                        <Field label="Endpoint URL" required>
                            <Input
                                autoComplete="url"
                                onChange={(e) => setEditUrl(e.target.value)}
                                required
                                value={editUrl}
                            />
                        </Field>
                    </div>
                    <SecretRefSelect
                        label="API key secret"
                        literalLabel="Keep the literal value authored in YAML"
                        noneLabel="No API key"
                        onChange={setEditApiKeySecret}
                        value={editApiKeySecret}
                    />
                </div>
            </FormDialog>
        </div>
    );
}
