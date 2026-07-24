import type { FormEvent } from "react";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { Connection } from "./types";
import { api } from "./api";
import { queryKeys, useConnections } from "./queries";
import { useErrorContext } from "./ErrorContext";
import { Button, Input, Select } from "./fluent";

type ConnectionApiFlavor = Connection["api_flavor"];
type ConnectionProviderType = Connection["provider_type"];
type ConnectionTransport = Connection["transport"];
type CredentialMode = "none" | "api_key" | "bearer_token";
type EditCredentialMode = "keep" | CredentialMode;
type EditHeadersMode = "keep" | "replace" | "clear";

const API_FLAVOR_OPTIONS: Array<{ value: ConnectionApiFlavor; label: string }> = [
    { value: "chat_completions", label: "Chat completions" },
    { value: "responses", label: "Responses" },
];

const PROVIDER_OPTIONS: Array<{ value: ConnectionProviderType; label: string }> = [
    { value: "openai", label: "OpenAI compatible" },
    { value: "azure", label: "Azure OpenAI" },
    { value: "anthropic", label: "Anthropic" },
];

const TRANSPORT_OPTIONS: Array<{ value: ConnectionTransport; label: string }> = [
    { value: "http", label: "HTTP" },
    { value: "websockets", label: "WebSockets" },
];

const apiFlavorLabel = (value: ConnectionApiFlavor) =>
    API_FLAVOR_OPTIONS.find((option) => option.value === value)?.label ?? value;

const providerLabel = (value: ConnectionProviderType) =>
    PROVIDER_OPTIONS.find((option) => option.value === value)?.label ?? value;

function parseHeaders(raw: string): Record<string, string> {
    if (!raw.trim()) return {};
    const value: unknown = JSON.parse(raw);
    if (
        typeof value !== "object"
        || value === null
        || Array.isArray(value)
        || Object.values(value).some((headerValue) => typeof headerValue !== "string")
    ) {
        throw new Error("Headers must be a JSON object whose values are strings");
    }
    return value as Record<string, string>;
}

export default function ConnectionsView() {
    const { data: connections = [] } = useConnections();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [showForm, setShowForm] = useState(false);
    const [formId, setFormId] = useState("");
    const [formName, setFormName] = useState("");
    const [formUrl, setFormUrl] = useState("");
    const [formProviderType, setFormProviderType] = useState<ConnectionProviderType>("openai");
    const [formApiFlavor, setFormApiFlavor] = useState<ConnectionApiFlavor>("chat_completions");
    const [formTransport, setFormTransport] = useState<ConnectionTransport>("http");
    const [formAzureApiVersion, setFormAzureApiVersion] = useState("");
    const [formCredentialMode, setFormCredentialMode] = useState<CredentialMode>("none");
    const [formCredential, setFormCredential] = useState("");
    const [formHeaders, setFormHeaders] = useState("");

    const [editingId, setEditingId] = useState<string | null>(null);
    const [editName, setEditName] = useState("");
    const [editUrl, setEditUrl] = useState("");
    const [editProviderType, setEditProviderType] = useState<ConnectionProviderType>("openai");
    const [editApiFlavor, setEditApiFlavor] = useState<ConnectionApiFlavor>("chat_completions");
    const [editTransport, setEditTransport] = useState<ConnectionTransport>("http");
    const [editAzureApiVersion, setEditAzureApiVersion] = useState("");
    const [editCredentialMode, setEditCredentialMode] = useState<EditCredentialMode>("keep");
    const [editCredential, setEditCredential] = useState("");
    const [editHeadersMode, setEditHeadersMode] = useState<EditHeadersMode>("keep");
    const [editHeaders, setEditHeaders] = useState("");

    const invalidate = () =>
        queryClient.invalidateQueries({ queryKey: queryKeys.connections });

    const createMutation = useMutation({
        mutationFn: api.createConnection,
        onSuccess: () => invalidate(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({ id, payload }: {
            id: string;
            payload: Parameters<typeof api.updateConnection>[1];
        }) => api.updateConnection(id, payload),
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
        try {
            const headers = parseHeaders(formHeaders);
            await createMutation.mutateAsync({
                connection_id: formId,
                name: formName,
                url: formUrl,
                provider_type: formProviderType,
                api_flavor: formApiFlavor,
                transport: formTransport,
                azure_api_version: formProviderType === "azure" && formAzureApiVersion
                    ? formAzureApiVersion
                    : undefined,
                api_key: formCredentialMode === "api_key" ? formCredential : undefined,
                bearer_token: formCredentialMode === "bearer_token" ? formCredential : undefined,
                headers: Object.keys(headers).length > 0 ? headers : undefined,
            });
            setFormId("");
            setFormName("");
            setFormUrl("");
            setFormProviderType("openai");
            setFormApiFlavor("chat_completions");
            setFormTransport("http");
            setFormAzureApiVersion("");
            setFormCredentialMode("none");
            setFormCredential("");
            setFormHeaders("");
            setShowForm(false);
        } catch (error) {
            reportError(error);
        }
    }

    function openEdit(conn: Connection) {
        setEditingId(conn.connection_id);
        setEditName(conn.name);
        setEditUrl(conn.url);
        setEditProviderType(conn.provider_type);
        setEditApiFlavor(conn.api_flavor);
        setEditTransport(conn.transport);
        setEditAzureApiVersion(conn.azure_api_version ?? "");
        setEditCredentialMode("keep");
        setEditCredential("");
        setEditHeadersMode("keep");
        setEditHeaders("");
    }

    function cancelEdit() {
        setEditingId(null);
        setEditName("");
        setEditUrl("");
        setEditProviderType("openai");
        setEditApiFlavor("chat_completions");
        setEditTransport("http");
        setEditAzureApiVersion("");
        setEditCredentialMode("keep");
        setEditCredential("");
        setEditHeadersMode("keep");
        setEditHeaders("");
    }

    async function handleEditSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        if (!editingId) return;
        try {
            const payload: Parameters<typeof api.updateConnection>[1] = {
                name: editName,
                url: editUrl,
                provider_type: editProviderType,
                api_flavor: editApiFlavor,
                transport: editTransport,
                azure_api_version: editProviderType === "azure"
                    ? (editAzureApiVersion || null)
                    : null,
            };
            if (editCredentialMode === "none") {
                payload.api_key = "";
                payload.bearer_token = "";
            } else if (editCredentialMode === "api_key") {
                payload.api_key = editCredential;
                payload.bearer_token = "";
            } else if (editCredentialMode === "bearer_token") {
                payload.api_key = "";
                payload.bearer_token = editCredential;
            }
            if (editHeadersMode === "clear") {
                payload.headers = {};
            } else if (editHeadersMode === "replace") {
                payload.headers = parseHeaders(editHeaders);
            }
            await updateMutation.mutateAsync({ id: editingId, payload });
            cancelEdit();
        } catch (error) {
            reportError(error);
        }
    }

    const editingConn = editingId
        ? (connections.find((connection) => connection.connection_id === editingId) ?? null)
        : null;

    const providerFields = (
        providerType: ConnectionProviderType,
        setProviderType: (value: ConnectionProviderType) => void,
        apiFlavor: ConnectionApiFlavor,
        setApiFlavor: (value: ConnectionApiFlavor) => void,
        transport: ConnectionTransport,
        setTransport: (value: ConnectionTransport) => void,
        azureApiVersion: string,
        setAzureApiVersion: (value: string) => void,
    ) => (
        <>
            <label>
                Provider
                <Select
                    required
                    value={providerType}
                    onChange={(event) => setProviderType(event.target.value as ConnectionProviderType)}
                >
                    {PROVIDER_OPTIONS.map((option) => (
                        <option key={option.value} value={option.value}>{option.label}</option>
                    ))}
                </Select>
            </label>
            <label>
                API Flavor
                <Select
                    required
                    value={apiFlavor}
                    onChange={(event) => setApiFlavor(event.target.value as ConnectionApiFlavor)}
                >
                    {API_FLAVOR_OPTIONS.map((option) => (
                        <option key={option.value} value={option.value}>{option.label}</option>
                    ))}
                </Select>
            </label>
            <label>
                Transport
                <Select
                    required
                    value={transport}
                    onChange={(event) => setTransport(event.target.value as ConnectionTransport)}
                >
                    {TRANSPORT_OPTIONS.map((option) => (
                        <option key={option.value} value={option.value}>{option.label}</option>
                    ))}
                </Select>
                {transport === "websockets" && apiFlavor !== "responses" && (
                    <span className="muted">WebSockets requires the Responses API flavor</span>
                )}
            </label>
            {providerType === "azure" && (
                <label>
                    Azure API Version
                    <Input
                        placeholder="2025-04-01-preview"
                        value={azureApiVersion}
                        onChange={(event) => setAzureApiVersion(event.target.value)}
                    />
                </label>
            )}
        </>
    );

    return (
        <div className="view-content management-view connections-management-view">
            <div className="view-header">
                <div>
                    <h2>Connections</h2>
                    <span className="muted">
                        {connections.length} endpoints ·{" "}
                        {connections.filter((connection) =>
                            connection.has_api_key || connection.has_bearer_token
                        ).length} authenticated
                    </span>
                </div>
                <div className="view-header-actions">
                    <Button onClick={() => setShowForm(!showForm)} type="button">
                        {showForm ? "Cancel" : "New Connection"}
                    </Button>
                </div>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={(event) => { void handleSubmit(event); }}>
                    <label>
                        Connection ID
                        <Input
                            autoComplete="username"
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="openai"
                            required
                            value={formId}
                            onChange={(event) => setFormId(event.target.value)}
                        />
                    </label>
                    <label>
                        Display Name
                        <Input
                            autoComplete="organization"
                            placeholder="OpenAI"
                            required
                            value={formName}
                            onChange={(event) => setFormName(event.target.value)}
                        />
                    </label>
                    <label>
                        URL
                        <Input
                            autoComplete="url"
                            placeholder="https://api.openai.com/v1"
                            required
                            value={formUrl}
                            onChange={(event) => setFormUrl(event.target.value)}
                        />
                    </label>
                    {providerFields(
                        formProviderType,
                        setFormProviderType,
                        formApiFlavor,
                        setFormApiFlavor,
                        formTransport,
                        setFormTransport,
                        formAzureApiVersion,
                        setFormAzureApiVersion,
                    )}
                    <label>
                        Authentication
                        <Select
                            value={formCredentialMode}
                            onChange={(event) => {
                                setFormCredentialMode(event.target.value as CredentialMode);
                                setFormCredential("");
                            }}
                        >
                            <option value="none">None (local provider)</option>
                            <option value="api_key">API key</option>
                            <option value="bearer_token">Bearer token</option>
                        </Select>
                    </label>
                    {formCredentialMode !== "none" && (
                        <label>
                            {formCredentialMode === "api_key" ? "API Key" : "Bearer Token"}
                            <Input
                                autoComplete="new-password"
                                required
                                type="password"
                                value={formCredential}
                                onChange={(event) => setFormCredential(event.target.value)}
                            />
                        </label>
                    )}
                    <label>
                        Additional Headers
                        <Input
                            autoComplete="new-password"
                            placeholder={'{"x-tenant-id":"..."}'}
                            type="password"
                            value={formHeaders}
                            onChange={(event) => setFormHeaders(event.target.value)}
                        />
                        <span className="muted">Optional JSON object. Header values are stored as secrets.</span>
                    </label>
                    <Button disabled={busy} type="submit">Create Connection</Button>
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
                                <span className={`status-badge ${
                                    conn.has_api_key || conn.has_bearer_token ? "active" : "stopped"
                                }`}>
                                    {conn.has_api_key
                                        ? "API key set"
                                        : conn.has_bearer_token
                                            ? "bearer set"
                                            : "no credential"}
                                </span>
                            </div>
                            <div className="card-meta">
                                <div><strong>URL:</strong>{" "}
                                    <span className="truncate-value" title={conn.url}>{conn.url}</span>
                                </div>
                                <div><strong>Provider:</strong> {providerLabel(conn.provider_type)}</div>
                                <div><strong>API Flavor:</strong> {apiFlavorLabel(conn.api_flavor)}</div>
                                <div><strong>Transport:</strong> {conn.transport}</div>
                                {conn.azure_api_version && (
                                    <div><strong>Azure API Version:</strong> {conn.azure_api_version}</div>
                                )}
                                <div><strong>Additional Headers:</strong>{" "}
                                    {conn.has_headers ? "set" : "not set"}
                                </div>
                            </div>
                            {editingId === conn.connection_id && (
                                <form className="create-form" onSubmit={(event) => {
                                    void handleEditSubmit(event);
                                }}>
                                    <label>
                                        Display Name
                                        <Input
                                            autoComplete="organization"
                                            required
                                            value={editName}
                                            onChange={(event) => setEditName(event.target.value)}
                                        />
                                    </label>
                                    <label>
                                        URL
                                        <Input
                                            autoComplete="url"
                                            required
                                            value={editUrl}
                                            onChange={(event) => setEditUrl(event.target.value)}
                                        />
                                    </label>
                                    {providerFields(
                                        editProviderType,
                                        setEditProviderType,
                                        editApiFlavor,
                                        setEditApiFlavor,
                                        editTransport,
                                        setEditTransport,
                                        editAzureApiVersion,
                                        setEditAzureApiVersion,
                                    )}
                                    <label>
                                        Authentication
                                        <Select
                                            value={editCredentialMode}
                                            onChange={(event) => {
                                                setEditCredentialMode(
                                                    event.target.value as EditCredentialMode,
                                                );
                                                setEditCredential("");
                                            }}
                                        >
                                            <option value="keep">Keep current</option>
                                            <option value="none">Clear credential</option>
                                            <option value="api_key">Replace with API key</option>
                                            <option value="bearer_token">Replace with bearer token</option>
                                        </Select>
                                        {editingConn && (
                                            <span className="muted">
                                                Current: {editingConn.has_api_key
                                                    ? "API key"
                                                    : editingConn.has_bearer_token
                                                        ? "bearer token"
                                                        : "none"}
                                            </span>
                                        )}
                                    </label>
                                    {editCredentialMode !== "keep"
                                        && editCredentialMode !== "none" && (
                                        <label>
                                            {editCredentialMode === "api_key"
                                                ? "New API Key"
                                                : "New Bearer Token"}
                                            <Input
                                                autoComplete="new-password"
                                                required
                                                type="password"
                                                value={editCredential}
                                                onChange={(event) =>
                                                    setEditCredential(event.target.value)}
                                            />
                                        </label>
                                    )}
                                    <label>
                                        Additional Headers
                                        <Select
                                            value={editHeadersMode}
                                            onChange={(event) =>
                                                setEditHeadersMode(
                                                    event.target.value as EditHeadersMode,
                                                )}
                                        >
                                            <option value="keep">Keep current</option>
                                            <option value="replace">Replace</option>
                                            <option value="clear">Clear</option>
                                        </Select>
                                        {editingConn && (
                                            <span className="muted">
                                                Current headers: {editingConn.has_headers ? "set" : "not set"}
                                            </span>
                                        )}
                                    </label>
                                    {editHeadersMode === "replace" && (
                                        <label>
                                            Header JSON
                                            <Input
                                                autoComplete="new-password"
                                                placeholder={'{"x-tenant-id":"..."}'}
                                                required
                                                type="password"
                                                value={editHeaders}
                                                onChange={(event) => setEditHeaders(event.target.value)}
                                            />
                                        </label>
                                    )}
                                    <div className="card-footer-actions">
                                        <Button disabled={busy} type="submit">Save Changes</Button>
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
