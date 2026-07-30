import type { FormEvent } from "react";
import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import type { Gateway, GatewayConfigField } from "./types";
import {
    queryKeys,
    useAgents,
    useGateways,
    useGatewaySchema,
    useGatewayTypes,
} from "./queries";
import { useErrorContext } from "./useErrorContext";
import { Button, Checkbox, Input, Select, Textarea } from "./fluent";

type SecretEntry = { key: string; value: string };

type SchemaOverrides = { gatewayType: string; values: Record<string, string> };

type EditDraft = {
    gatewayId: string;
    name: string;
    agentId: string;
    enabled: boolean;
    extraEnv: string;
    schemaValues: Record<string, string>;
    newSecrets: SecretEntry[];
};

const NO_OVERRIDES: SchemaOverrides = { gatewayType: "", values: {} };
const EMPTY_VALUES: Record<string, string> = {};
const EMPTY_SECRETS: SecretEntry[] = [];

function secretsToRecord(entries: SecretEntry[]): Record<string, string> {
    const record: Record<string, string> = {};
    for (const entry of entries) {
        const key = entry.key.trim();
        if (key) record[key] = entry.value;
    }
    return record;
}

function mergeEnvLines(
    schemaFields: GatewayConfigField[],
    schemaValues: Record<string, string>,
    extraEnv: string,
): string {
    const lines: string[] = [];
    const schemaEnvKeys = new Set(
        schemaFields.filter((f) => f.kind === "env").map((f) => f.key),
    );
    for (const field of schemaFields) {
        if (field.kind !== "env") continue;
        const value = schemaValues[field.key]?.trim();
        if (value) {
            lines.push(`${field.key}=${value}`);
        }
    }
    // Strip any line in the free-form textarea whose key duplicates a
    // schema-managed env field, so the labelled input always wins and we
    // never emit two assignments for the same key.
    const filteredExtra = extraEnv
        .split("\n")
        .filter((line) => {
            const match = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=/.exec(line);
            return !match || !schemaEnvKeys.has(match[1]);
        })
        .join("\n")
        .trim();
    if (filteredExtra) {
        lines.push(filteredExtra);
    }
    return lines.join("\n");
}

function mergeSecrets(
    schemaFields: GatewayConfigField[],
    schemaValues: Record<string, string>,
    extraSecrets: SecretEntry[],
): Record<string, string> {
    const merged: Record<string, string> = {};
    for (const field of schemaFields) {
        if (field.kind !== "secret") continue;
        const value = schemaValues[field.key];
        if (value && value.length > 0) {
            merged[field.key] = value;
        }
    }
    Object.assign(merged, secretsToRecord(extraSecrets));
    return merged;
}

/** Parse `KEY=value` lines (skipping blanks and `#` comments) into a map. */
function parseEnvVarsString(text: string): Record<string, string> {
    const out: Record<string, string> = {};
    for (const raw of text.split("\n")) {
        const line = raw.trim();
        if (!line || line.startsWith("#")) continue;
        const eq = line.indexOf("=");
        if (eq <= 0) continue;
        const key = line.slice(0, eq).trim();
        let value = line.slice(eq + 1).trim();
        // Strip a single pair of matching surrounding quotes.
        if (
            value.length >= 2
            && ((value.startsWith("\"") && value.endsWith("\""))
                || (value.startsWith("'") && value.endsWith("'")))
        ) {
            value = value.slice(1, -1);
        }
        out[key] = value;
    }
    return out;
}

/**
 * Split a stored `env_vars` blob into (a) values for schema-managed
 * env fields and (b) the leftover lines that should populate the
 * free-form textarea.
 */
function splitEnvForEdit(
    envVars: string,
    schemaFields: GatewayConfigField[],
): { schemaValues: Record<string, string>; extraEnv: string } {
    const parsed = parseEnvVarsString(envVars);
    const schemaEnvKeys = new Set(
        schemaFields.filter((f) => f.kind === "env").map((f) => f.key),
    );
    const schemaValues: Record<string, string> = {};
    for (const f of schemaFields) {
        if (f.kind === "env") {
            schemaValues[f.key] = parsed[f.key] ?? f.default ?? "";
        } else {
            // Secret values are never returned by the API; leave blank
            // so the user can rotate them without seeing a stale value.
            schemaValues[f.key] = "";
        }
    }
    const extraLines: string[] = [];
    for (const raw of envVars.split("\n")) {
        const line = raw.trim();
        if (!line || line.startsWith("#")) continue;
        const eq = line.indexOf("=");
        if (eq <= 0) continue;
        const key = line.slice(0, eq).trim();
        if (!schemaEnvKeys.has(key)) {
            extraLines.push(raw);
        }
    }
    return { schemaValues, extraEnv: extraLines.join("\n") };
}

export default function GatewaysView() {
    const { data: gateways = [] } = useGateways();
    const { data: agents = [], isLoading: agentsLoading } = useAgents();
    const { data: gatewayTypes = [] } = useGatewayTypes();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();
    const validAgentIds = useMemo(
        () => new Set(agents.map((agent) => agent.agent_id)),
        [agents],
    );

    const [showForm, setShowForm] = useState(false);
    const [gatewayId, setGatewayId] = useState("");
    const [gatewayName, setGatewayName] = useState("");
    const [selectedGatewayType, setGatewayType] = useState("");
    const [selectedAgentId, setAgentId] = useState("");
    const [enabled, setEnabled] = useState(false);
    const [envVars, setEnvVars] = useState("");
    const [newSecrets, setNewSecrets] = useState<SecretEntry[]>([]);
    const [schemaOverrides, setSchemaOverrides] = useState<SchemaOverrides>(NO_OVERRIDES);
    const [expandedGatewayId, setExpandedGatewayId] = useState<string | null>(null);

    // Fall back to the first available option until the user picks one explicitly.
    const gatewayType = gatewayTypes.includes(selectedGatewayType)
        ? selectedGatewayType
        : (gatewayTypes[0] ?? "");
    const agentId = validAgentIds.has(selectedAgentId)
        ? selectedAgentId
        : (agents[0]?.agent_id ?? "");

    // --- Edit-form state (mirrors the create-form state above, but
    // scoped to a single gateway being edited inline). ---
    const [editingGatewayId, setEditingGatewayId] = useState<string | null>(null);
    // Only populated once the user actually edits something; until then the
    // form values are derived from the gateway being edited.
    const [editDraft, setEditDraft] = useState<EditDraft | null>(null);

    const editingGateway = editingGatewayId
        ? (gateways.find((g) => g.gateway_id === editingGatewayId) ?? null)
        : null;

    const schemaQuery = useGatewaySchema(gatewayType || null);
    const schema = schemaQuery.data ?? null;
    const schemaLoading = schemaQuery.isFetching;

    const editSchemaQuery = useGatewaySchema(editingGateway?.gateway_type ?? null);
    const editSchema = editSchemaQuery.data ?? null;
    const editSchemaLoading = editSchemaQuery.isFetching;

    // Schema field values default to the schema's own defaults; user edits are
    // tracked as overrides so switching gateway type resets back to defaults.
    const schemaValues = useMemo(() => {
        if (!schema) return {};
        const overrides =
            schemaOverrides.gatewayType === gatewayType ? schemaOverrides.values : {};
        const values: Record<string, string> = {};
        for (const field of schema.fields) {
            values[field.key] = overrides[field.key] ?? field.default ?? "";
        }
        return values;
    }, [schema, schemaOverrides, gatewayType]);

    // Derive the edit form from the gateway until the user changes something.
    // The schema may still be loading when the user clicks Edit, so this can't
    // be snapshotted eagerly.
    const editValues = useMemo<EditDraft | null>(() => {
        if (!editingGateway || !editSchema) return null;
        if (editDraft && editDraft.gatewayId === editingGateway.gateway_id) {
            return editDraft;
        }
        const { schemaValues: sv, extraEnv } = splitEnvForEdit(
            editingGateway.env_vars,
            editSchema.fields,
        );
        return {
            gatewayId: editingGateway.gateway_id,
            name: editingGateway.name,
            agentId: editingGateway.agent_id,
            enabled: editingGateway.enabled,
            extraEnv,
            schemaValues: sv,
            newSecrets: [],
        };
    }, [editingGateway, editSchema, editDraft]);

    const editName = editValues?.name ?? "";
    const editAgentId = editValues?.agentId ?? "";
    const editEnabled = editValues?.enabled ?? false;
    const editExtraEnv = editValues?.extraEnv ?? "";
    const editSchemaValues = editValues?.schemaValues ?? EMPTY_VALUES;
    const editNewSecrets = editValues?.newSecrets ?? EMPTY_SECRETS;

    function patchEditDraft(patch: Partial<EditDraft>) {
        if (!editValues) return;
        setEditDraft({ ...editValues, ...patch });
    }

    const logsQuery = useQuery({
        queryKey: expandedGatewayId
            ? queryKeys.gatewayLogs(expandedGatewayId)
            : (["gateways", "__none__", "logs"] as const),
        queryFn: () => api.gatewayLogs(expandedGatewayId as string),
        enabled: expandedGatewayId !== null,
    });

    const invalidateGateways = () =>
        queryClient.invalidateQueries({ queryKey: queryKeys.gateways });

    const createMutation = useMutation({
        mutationFn: (payload: {
            gateway_id: string;
            name: string;
            gateway_type: string;
            agent_id: string;
            enabled: boolean;
            env_vars: string;
            secrets: Record<string, string>;
        }) => api.createGateway(payload),
        onSuccess: () => invalidateGateways(),
        onError: reportError,
    });

    const updateMutation = useMutation({
        mutationFn: ({
            gatewayId,
            payload,
        }: {
            gatewayId: string;
            payload: {
                name?: string;
                agent_id?: string;
                enabled?: boolean;
                env_vars?: string;
                secrets?: Record<string, string>;
            };
        }) => api.updateGateway(gatewayId, payload),
        onSuccess: () => invalidateGateways(),
        onError: reportError,
    });

    const deleteMutation = useMutation({
        mutationFn: (gatewayId: string) => api.deleteGateway(gatewayId),
        onSuccess: () => invalidateGateways(),
        onError: reportError,
    });

    const startMutation = useMutation({
        mutationFn: (gatewayId: string) => api.startGateway(gatewayId),
        onSuccess: () => invalidateGateways(),
        onError: reportError,
    });

    const stopMutation = useMutation({
        mutationFn: (gatewayId: string) => api.stopGateway(gatewayId),
        onSuccess: () => invalidateGateways(),
        onError: reportError,
    });

    const busy =
        createMutation.isPending
        || updateMutation.isPending
        || deleteMutation.isPending
        || startMutation.isPending
        || stopMutation.isPending;

    function updateSecret(index: number, field: "key" | "value", value: string) {
        setNewSecrets((prev) => prev.map((s, i) => (i === index ? { ...s, [field]: value } : s)));
    }

    function addSecret() {
        setNewSecrets((prev) => [...prev, { key: "", value: "" }]);
    }

    function removeSecret(index: number) {
        setNewSecrets((prev) => prev.filter((_, i) => i !== index));
    }

    async function handleSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        const fields = schema?.fields ?? [];
        await createMutation.mutateAsync({
            gateway_id: gatewayId,
            name: gatewayName,
            gateway_type: gatewayType,
            agent_id: agentId,
            enabled,
            env_vars: mergeEnvLines(fields, schemaValues, envVars),
            secrets: mergeSecrets(fields, schemaValues, newSecrets),
        });
        setGatewayId("");
        setGatewayName("");
        setEnabled(false);
        setEnvVars("");
        setNewSecrets([]);
        setSchemaOverrides(NO_OVERRIDES);
        setShowForm(false);
    }

    function updateSchemaValue(key: string, value: string) {
        setSchemaOverrides((prev) => ({
            gatewayType,
            values: {
                ...(prev.gatewayType === gatewayType ? prev.values : {}),
                [key]: value,
            },
        }));
    }

    // --- Edit helpers ---
    function openEdit(gateway: Gateway) {
        setEditingGatewayId(gateway.gateway_id);
        setEditDraft(null);
    }

    function cancelEdit() {
        setEditingGatewayId(null);
        setEditDraft(null);
    }

    function updateEditSchemaValue(key: string, value: string) {
        patchEditDraft({ schemaValues: { ...editSchemaValues, [key]: value } });
    }

    function updateEditSecret(index: number, field: "key" | "value", value: string) {
        patchEditDraft({
            newSecrets: editNewSecrets.map((s, i) =>
                i === index ? { ...s, [field]: value } : s,
            ),
        });
    }

    function addEditSecret() {
        patchEditDraft({ newSecrets: [...editNewSecrets, { key: "", value: "" }] });
    }

    function removeEditSecret(index: number) {
        patchEditDraft({ newSecrets: editNewSecrets.filter((_, i) => i !== index) });
    }

    async function handleEditSubmit(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        if (!editingGateway || !editSchema) return;
        if (!validAgentIds.has(editAgentId)) return;
        const fields = editSchema.fields;
        // `secrets` is sent as an OVERLAY (see service.update_gateway):
        // only keys with a non-empty value are included, so existing
        // unmodified secrets are preserved server-side.
        await updateMutation.mutateAsync({
            gatewayId: editingGateway.gateway_id,
            payload: {
                name: editName,
                agent_id: editAgentId,
                enabled: editEnabled,
                env_vars: mergeEnvLines(fields, editSchemaValues, editExtraEnv),
                secrets: mergeSecrets(fields, editSchemaValues, editNewSecrets),
            },
        });
        cancelEdit();
    }

    function handleToggleLogs(gateway: Gateway) {
        if (expandedGatewayId === gateway.gateway_id) {
            setExpandedGatewayId(null);
            return;
        }
        setExpandedGatewayId(gateway.gateway_id);
    }

    return (
        <div className="view-content management-view gateways-management-view">
            <div className="view-header">
                <div>
                    <h2>Gateways</h2>
                    <span className="muted">
                        {gateways.length} total · {gateways.filter((gateway) => gateway.status === "running").length} running
                    </span>
                </div>
                <div className="view-header-actions">
                    <Button onClick={() => setShowForm(!showForm)} type="button">
                        {showForm ? "Cancel" : "New Gateway"}
                    </Button>
                </div>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={(e) => { void handleSubmit(e); }}>
                    <label>
                        Gateway ID
                        <Input
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="echo-bridge"
                            required
                            value={gatewayId}
                            onChange={(e) => setGatewayId(e.target.value)}
                        />
                    </label>
                    <label>
                        Name
                        <Input
                            placeholder="My Echo Gateway"
                            required
                            value={gatewayName}
                            onChange={(e) => setGatewayName(e.target.value)}
                        />
                    </label>
                    <label>
                        Type
                        <Select
                            value={gatewayType}
                            onChange={(e) => setGatewayType(e.target.value)}
                        >
                            {gatewayTypes.map((type) => (
                                <option key={type} value={type}>
                                    {type}
                                </option>
                            ))}
                        </Select>
                    </label>
                    <label>
                        Agent
                        <Select
                            value={agentId}
                            onChange={(e) => setAgentId(e.target.value)}
                            required
                        >
                            <option disabled value="">
                                Select an agent
                            </option>
                            {agents.map((agent) => (
                                <option key={agent.agent_id} value={agent.agent_id}>
                                    {agent.name} ({agent.agent_id})
                                </option>
                            ))}
                        </Select>
                    </label>
                    <Checkbox
                        checked={enabled}
                        className="checkbox-label"
                        label="Auto-start on boot"
                        onChange={(_, data) => setEnabled(data.checked === true)}
                    />
                    {schema && schema.fields.length > 0 && (
                        <fieldset className="schema-fields">
                            <legend>Gateway environment variables</legend>
                            {schema.fields.map((f) => (
                                <label key={f.key}>
                                    {f.label}
                                    {f.required && <span aria-hidden="true"> *</span>}
                                    <Input
                                        autoComplete={f.kind === "secret" ? "new-password" : undefined}
                                        type={f.kind === "secret" ? "password" : "text"}
                                        required={f.required}
                                        placeholder={f.placeholder ?? f.default ?? ""}
                                        value={schemaValues[f.key] ?? ""}
                                        onChange={(e) =>
                                            updateSchemaValue(f.key, e.target.value)
                                        }
                                    />
                                    {f.description && (
                                        <small className="field-help">{f.description}</small>
                                    )}
                                </label>
                            ))}
                        </fieldset>
                    )}
                    {schemaLoading && (
                        <small className="field-help">Loading gateway schema…</small>
                    )}
                    <label>
                        Other environment variables (.env format)
                        <Textarea
                            placeholder="EXTRA_VAR=value"
                            rows={4}
                            value={envVars}
                            onChange={(e) => setEnvVars(e.target.value)}
                        />
                    </label>
                    <div className="skill-files-section">
                        <div className="skill-files-header">
                            <span className="skill-files-label">
                                Other secrets (passed as env)
                            </span>
                            <Button
                                className="secondary-button small"
                                onClick={addSecret}
                                type="button"
                            >
                                + Add Secret
                            </Button>
                        </div>
                        {newSecrets.map((secret, index) => (
                            <div className="skill-file-entry-header" key={index}>
                                <Input
                                    placeholder="KEY"
                                    value={secret.key}
                                    onChange={(e) => updateSecret(index, "key", e.target.value)}
                                />
                                <Input
                                    autoComplete="new-password"
                                    placeholder="value"
                                    type="password"
                                    value={secret.value}
                                    onChange={(e) => updateSecret(index, "value", e.target.value)}
                                />
                                <Button
                                    className="icon-button danger-button"
                                    onClick={() => removeSecret(index)}
                                    type="button"
                                    title="Remove secret"
                                >
                                    ×
                                </Button>
                            </div>
                        ))}
                    </div>
                    <Button disabled={busy || !agentId} type="submit">
                        Create Gateway
                    </Button>
                </form>
            )}

            <div className="card-grid management-card-grid">
                {gateways.map((gateway) => {
                    const gatewayAgent = agents.find(
                        (agent) => agent.agent_id === gateway.agent_id,
                    );
                    const hasValidAgent = validAgentIds.has(gateway.agent_id);
                    const showMissingAgent = !agentsLoading && !hasValidAgent;
                    const editHasValidAgent = validAgentIds.has(editAgentId);
                    const editShowsMissingAgent =
                        Boolean(editAgentId) && !agentsLoading && !editHasValidAgent;
                    return (
                    <div className="card management-card" key={gateway.gateway_id}>
                        <div className="card-body">
                            <div className="management-card-heading">
                                <div className="management-title-block">
                                    <h3>{gateway.name}</h3>
                                    <code className="management-id">{gateway.gateway_id}</code>
                                </div>
                                <div className="badge-row">
                                    {showMissingAgent && (
                                        <span className="status-badge invalid">invalid</span>
                                    )}
                                    <span className={`status-badge ${gateway.status}`}>
                                        {gateway.status}
                                    </span>
                                </div>
                            </div>
                            <div className="card-meta management-meta">
                                <div>
                                    <strong>ID</strong>
                                    <span className="truncate-value">{gateway.gateway_id}</span>
                                </div>
                                <div>
                                    <strong>Type</strong>
                                    <span>{gateway.gateway_type}</span>
                                </div>
                                <div className={showMissingAgent ? "error-text" : undefined}>
                                    <strong>Agent</strong>
                                    <span className="truncate-value">
                                        {gatewayAgent
                                            ? `${gatewayAgent.name} (${gatewayAgent.agent_id})`
                                            : `${gateway.agent_id}${showMissingAgent ? " (missing)" : ""}`}
                                    </span>
                                </div>
                                <div>
                                    <strong>Enabled</strong>
                                    <span>{gateway.enabled ? "yes" : "no"}</span>
                                </div>
                                {gateway.container_name && (
                                    <div>
                                        <strong>Container</strong>
                                        <span className="truncate-value" title={gateway.container_name}>{gateway.container_name}</span>
                                    </div>
                                )}
                                {gateway.secret_keys.length > 0 && (
                                    <div>
                                        <strong>Secrets</strong>
                                        <span className="truncate-value">{gateway.secret_keys.join(", ")}</span>
                                    </div>
                                )}
                                {gateway.last_error && (
                                    <div className="error-text">
                                        <strong>Error</strong>
                                        <span className="truncate-value">{gateway.last_error}</span>
                                    </div>
                                )}
                            </div>
                            {showMissingAgent && (
                                <div className="warning-box">
                                    This gateway points to a deleted agent. Edit it and select an
                                    existing agent before starting or enabling it.
                                </div>
                            )}
                            {expandedGatewayId === gateway.gateway_id && logsQuery.data && (
                                <pre className="skill-file-content management-log-block">{logsQuery.data.lines.join("\n")}</pre>
                            )}
                            {editingGatewayId === gateway.gateway_id && (
                                <form
                                    className="create-form"
                                    onSubmit={(e) => { void handleEditSubmit(e); }}
                                >
                                    {gateway.status === "running" && (
                                        <small className="field-help">
                                            This gateway is running. Saving will tear down its
                                            container and respawn it with the new configuration.
                                        </small>
                                    )}
                                    <label>
                                        Name
                                        <Input
                                            required
                                            value={editName}
                                            onChange={(e) => patchEditDraft({ name: e.target.value })}
                                        />
                                    </label>
                                    <label>
                                        Agent
                                        <Select
                                            value={editAgentId}
                                            onChange={(e) => patchEditDraft({ agentId: e.target.value })}
                                            required
                                        >
                                            <option disabled value="">
                                                Select an agent
                                            </option>
                                            {editShowsMissingAgent && (
                                                <option disabled value={editAgentId}>
                                                    Missing agent ({editAgentId})
                                                </option>
                                            )}
                                            {agents.map((agent) => (
                                                <option key={agent.agent_id} value={agent.agent_id}>
                                                    {agent.name} ({agent.agent_id})
                                                </option>
                                            ))}
                                        </Select>
                                        {editShowsMissingAgent && (
                                            <small className="field-help error-text">
                                                The currently assigned agent no longer exists. Select
                                                an existing agent to repair this gateway.
                                            </small>
                                        )}
                                    </label>
                                    <Checkbox
                                        checked={editEnabled}
                                        className="checkbox-label"
                                        label="Auto-start on boot"
                                        onChange={(_, data) => patchEditDraft({ enabled: data.checked === true })}
                                    />
                                    {editSchema && editSchema.fields.length > 0 && (
                                        <fieldset className="schema-fields">
                                            <legend>Gateway environment variables</legend>
                                            {editSchema.fields.map((f) => {
                                                const isExistingSecret =
                                                    f.kind === "secret"
                                                    && gateway.secret_keys.includes(f.key);
                                                return (
                                                    <label key={f.key}>
                                                        {f.label}
                                                        {f.required && <span aria-hidden="true"> *</span>}
                                                        <Input
                                                            autoComplete={f.kind === "secret" ? "new-password" : undefined}
                                                            type={f.kind === "secret" ? "password" : "text"}
                                                            // Don't enforce required on existing secrets:
                                                            // empty means "keep current value".
                                                            required={f.required && !isExistingSecret}
                                                            placeholder={
                                                                isExistingSecret
                                                                    ? "(leave blank to keep current value)"
                                                                    : (f.placeholder ?? f.default ?? "")
                                                            }
                                                            value={editSchemaValues[f.key] ?? ""}
                                                            onChange={(e) =>
                                                                updateEditSchemaValue(f.key, e.target.value)
                                                            }
                                                        />
                                                        {f.description && (
                                                            <small className="field-help">{f.description}</small>
                                                        )}
                                                    </label>
                                                );
                                            })}
                                        </fieldset>
                                    )}
                                    {editSchemaLoading && (
                                        <small className="field-help">Loading gateway schema…</small>
                                    )}
                                    <label>
                                        Other environment variables (.env format)
                                        <Textarea
                                            placeholder="EXTRA_VAR=value"
                                            rows={4}
                                            value={editExtraEnv}
                                            onChange={(e) => patchEditDraft({ extraEnv: e.target.value })}
                                        />
                                    </label>
                                    <div className="skill-files-section">
                                        <div className="skill-files-header">
                                            <span className="skill-files-label">
                                                Other secrets (passed as env)
                                            </span>
                                            <Button
                                                className="secondary-button small"
                                                onClick={addEditSecret}
                                                type="button"
                                            >
                                                + Add Secret
                                            </Button>
                                        </div>
                                        {editNewSecrets.map((secret, index) => (
                                            <div className="skill-file-entry-header" key={index}>
                                                <Input
                                                    placeholder="KEY"
                                                    value={secret.key}
                                                    onChange={(e) =>
                                                        updateEditSecret(index, "key", e.target.value)
                                                    }
                                                />
                                                <Input
                                                    autoComplete="new-password"
                                                    placeholder="value"
                                                    type="password"
                                                    value={secret.value}
                                                    onChange={(e) =>
                                                        updateEditSecret(index, "value", e.target.value)
                                                    }
                                                />
                                                <Button
                                                    className="icon-button danger-button"
                                                    onClick={() => removeEditSecret(index)}
                                                    type="button"
                                                    title="Remove secret"
                                                >
                                                    ×
                                                </Button>
                                            </div>
                                        ))}
                                    </div>
                                    <div className="card-footer-actions">
                                        <Button
                                            disabled={
                                                busy
                                                || !editAgentId
                                                || !editSchema
                                                || agentsLoading
                                                || !editHasValidAgent
                                            }
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
                            <Button
                                className="secondary-button small"
                                disabled={logsQuery.isFetching && expandedGatewayId === gateway.gateway_id}
                                onClick={() => handleToggleLogs(gateway)}
                                type="button"
                            >
                                {expandedGatewayId === gateway.gateway_id
                                    ? "Hide Logs"
                                    : "View Logs"}
                            </Button>
                            <div className="card-footer-actions">
                                {gateway.status === "running" ? (
                                    <Button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => stopMutation.mutate(gateway.gateway_id)}
                                        type="button"
                                    >
                                        Stop
                                    </Button>
                                ) : (
                                    <Button
                                        className="small"
                                        disabled={busy || agentsLoading || !hasValidAgent}
                                        onClick={() => startMutation.mutate(gateway.gateway_id)}
                                        type="button"
                                    >
                                        Start
                                    </Button>
                                )}
                                <Button
                                    className="secondary-button small"
                                    disabled={
                                        busy
                                        || agentsLoading
                                        || (!hasValidAgent && !gateway.enabled)
                                    }
                                    onClick={() =>
                                        updateMutation.mutate({
                                            gatewayId: gateway.gateway_id,
                                            payload: { enabled: !gateway.enabled },
                                        })
                                    }
                                    type="button"
                                >
                                    {gateway.enabled ? "Disable" : "Enable"}
                                </Button>
                                <Button
                                    className="secondary-button small"
                                    disabled={busy}
                                    onClick={() => openEdit(gateway)}
                                    type="button"
                                    title="Edit gateway configuration. Running gateways will be restarted to pick up changes."
                                >
                                    Edit
                                </Button>
                                <Button
                                    className="danger-button small"
                                    disabled={busy}
                                    onClick={() => deleteMutation.mutate(gateway.gateway_id)}
                                    type="button"
                                >
                                    Delete
                                </Button>
                            </div>
                        </div>
                    </div>
                    );
                })}
                {gateways.length === 0 && (
                    <div className="empty-state">
                        No gateways yet. Create one to bridge an external system to an agent.
                    </div>
                )}
            </div>
        </div>
    );
}
