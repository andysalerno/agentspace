import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
    Add20Regular,
    ArrowDownload20Regular,
    Delete20Regular,
    Dismiss20Regular,
    Edit20Regular,
    Play20Regular,
    PlugDisconnected24Regular,
    Power20Regular,
    Stop20Regular,
    TextBulletListLtr20Regular,
} from "@fluentui/react-icons";
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
import {
    Button,
    Checkbox,
    Field,
    Input,
    MessageBar,
    MessageBarBody,
    Select,
    Textarea,
} from "./fluent";
import {
    EmptyState,
    FormDialog,
    LogsDialog,
    RowActions,
    StatusBadge,
    ViewHeader,
} from "./ui";
import { statusTone } from "./status";

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
    const [logsGatewayId, setLogsGatewayId] = useState<string | null>(null);

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
        queryKey: logsGatewayId
            ? queryKeys.gatewayLogs(logsGatewayId)
            : (["gateways", "__none__", "logs"] as const),
        queryFn: () => api.gatewayLogs(logsGatewayId as string),
        enabled: logsGatewayId !== null,
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

    async function handleSubmit() {
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

    async function handleEditSubmit() {
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

    const runningCount = gateways.filter((gateway) => gateway.status === "running").length;

    function renderSchemaFields(
        fields: GatewayConfigField[],
        values: Record<string, string>,
        onChange: (key: string, value: string) => void,
        existingSecretKeys: string[] = [],
    ) {
        return fields.map((field) => {
            const isExistingSecret = field.kind === "secret"
                && existingSecretKeys.includes(field.key);
            return (
                <Field
                    className="span-2"
                    hint={field.description ?? undefined}
                    key={field.key}
                    label={field.label}
                    required={field.required}
                >
                    <Input
                        autoComplete={field.kind === "secret" ? "new-password" : undefined}
                        onChange={(e) => onChange(field.key, e.target.value)}
                        placeholder={isExistingSecret
                            ? "Leave blank to keep the current value"
                            : (field.placeholder ?? field.default ?? "")}
                        required={field.required && !isExistingSecret}
                        type={field.kind === "secret" ? "password" : "text"}
                        value={values[field.key] ?? ""}
                    />
                </Field>
            );
        });
    }

    function renderSecretRows(
        entries: SecretEntry[],
        onUpdate: (index: number, field: "key" | "value", value: string) => void,
        onRemove: (index: number) => void,
        onAdd: () => void,
    ) {
        return (
            <fieldset className="field-group span-2">
                <legend>Additional secrets</legend>
                <span className="field-group-help">
                    Passed to the gateway container as environment variables.
                </span>
                {entries.map((secret, index) => (
                    <div className="mount-row" key={index}>
                        <Input
                            onChange={(e) => onUpdate(index, "key", e.target.value)}
                            placeholder="KEY"
                            value={secret.key}
                        />
                        <Input
                            autoComplete="new-password"
                            onChange={(e) => onUpdate(index, "value", e.target.value)}
                            placeholder="value"
                            type="password"
                            value={secret.value}
                        />
                        <Button
                            appearance="subtle"
                            aria-label="Remove secret"
                            icon={<Dismiss20Regular />}
                            onClick={() => onRemove(index)}
                            type="button"
                        />
                    </div>
                ))}
                <div className="form-actions">
                    <Button icon={<Add20Regular />} onClick={onAdd} size="small" type="button">
                        Add secret
                    </Button>
                </div>
            </fieldset>
        );
    }

    const logsGateway = logsGatewayId
        ? (gateways.find((g) => g.gateway_id === logsGatewayId) ?? null)
        : null;

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
                        New gateway
                    </Button>
                }
                description={`${gateways.length} configured, ${runningCount} running`}
                title="Gateways"
            />
            <div className="view-body">
                {gateways.length === 0
                    ? (
                        <EmptyState
                            action={
                                <Button appearance="primary" onClick={() => setShowForm(true)}>
                                    New gateway
                                </Button>
                            }
                            description="A gateway bridges an external system such as Slack or GitHub to one of your agents."
                            icon={<PlugDisconnected24Regular />}
                            title="No gateways yet"
                        />
                    )
                    : (
                        <div className="table-container">
                            <div className="table-scroll">
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th>Gateway</th>
                                            <th>Type</th>
                                            <th>Status</th>
                                            <th>Agent</th>
                                            <th>Auto-start</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {gateways.map((gateway) => {
                                            const gatewayAgent = agents.find(
                                                (agent) => agent.agent_id === gateway.agent_id,
                                            );
                                            const hasValidAgent = validAgentIds.has(gateway.agent_id);
                                            const showMissingAgent = !agentsLoading && !hasValidAgent;
                                            const running = gateway.status === "running";
                                            return (
                                                <tr key={gateway.gateway_id}>
                                                    <td>
                                                        <div className="cell-identity">
                                                            <span className="cell-identity-name">
                                                                {gateway.name}
                                                            </span>
                                                            <span className="cell-identity-id">
                                                                {gateway.gateway_id}
                                                            </span>
                                                        </div>
                                                    </td>
                                                    <td>{gateway.gateway_type}</td>
                                                    <td>
                                                        <StatusBadge
                                                            label={gateway.status}
                                                            tone={statusTone(gateway.status)}
                                                        />
                                                        {gateway.last_error && (
                                                            <div
                                                                className="error-text truncate"
                                                                title={gateway.last_error}
                                                            >
                                                                {gateway.last_error}
                                                            </div>
                                                        )}
                                                    </td>
                                                    <td>
                                                        {showMissingAgent
                                                            ? (
                                                                <span className="error-text">
                                                                    {gateway.agent_id} (missing)
                                                                </span>
                                                            )
                                                            : (gatewayAgent?.name ?? gateway.agent_id)}
                                                    </td>
                                                    <td className="muted">
                                                        {gateway.enabled ? "On" : "Off"}
                                                    </td>
                                                    <td className="actions-cell">
                                                        <RowActions
                                                            items={[
                                                                {
                                                                    key: "logs",
                                                                    label: "View logs",
                                                                    icon: <TextBulletListLtr20Regular />,
                                                                    onClick: () =>
                                                                        setLogsGatewayId(gateway.gateway_id),
                                                                },
                                                                {
                                                                    key: "toggle",
                                                                    label: gateway.enabled
                                                                        ? "Disable auto-start"
                                                                        : "Enable auto-start",
                                                                    icon: <Power20Regular />,
                                                                    disabled: busy
                                                                        || agentsLoading
                                                                        || (!hasValidAgent && !gateway.enabled),
                                                                    onClick: () =>
                                                                        updateMutation.mutate({
                                                                            gatewayId: gateway.gateway_id,
                                                                            payload: { enabled: !gateway.enabled },
                                                                        }),
                                                                },
                                                                {
                                                                    key: "edit",
                                                                    label: "Edit configuration",
                                                                    icon: <Edit20Regular />,
                                                                    disabled: busy,
                                                                    onClick: () => openEdit(gateway),
                                                                },
                                                                {
                                                                    key: "export",
                                                                    label: "Export YAML",
                                                                    icon: <ArrowDownload20Regular />,
                                                                    onClick: () => {
                                                                        void api.downloadConfigResource(
                                                                            "gateway",
                                                                            gateway.gateway_id,
                                                                        ).catch(reportError);
                                                                    },
                                                                },
                                                                {
                                                                    key: "delete",
                                                                    label: "Delete",
                                                                    icon: <Delete20Regular />,
                                                                    destructive: true,
                                                                    disabled: busy,
                                                                    confirm:
                                                                        `Delete the gateway "${gateway.name}"? This cannot be undone.`,
                                                                    onClick: () =>
                                                                        deleteMutation.mutate(gateway.gateway_id),
                                                                },
                                                            ]}
                                                            primary={running
                                                                ? {
                                                                    key: "stop",
                                                                    label: "Stop",
                                                                    icon: <Stop20Regular />,
                                                                    disabled: busy,
                                                                    onClick: () =>
                                                                        stopMutation.mutate(gateway.gateway_id),
                                                                }
                                                                : {
                                                                    key: "start",
                                                                    label: "Start",
                                                                    icon: <Play20Regular />,
                                                                    disabled: busy || agentsLoading
                                                                        || !hasValidAgent,
                                                                    onClick: () =>
                                                                        startMutation.mutate(gateway.gateway_id),
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
                busy={busy || !agentId}
                onOpenChange={setShowForm}
                onSubmit={() => {
                    void handleSubmit();
                }}
                open={showForm}
                submitLabel="Create gateway"
                title="New gateway"
            >
                <div className="form-grid">
                    <Field hint="Lowercase letters and dashes." label="Gateway ID" required>
                        <Input
                            onChange={(e) => setGatewayId(e.target.value)}
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="echo-bridge"
                            required
                            value={gatewayId}
                        />
                    </Field>
                    <Field label="Name" required>
                        <Input
                            onChange={(e) => setGatewayName(e.target.value)}
                            placeholder="My echo gateway"
                            required
                            value={gatewayName}
                        />
                    </Field>
                    <Field label="Type">
                        <Select
                            onChange={(e) => setGatewayType(e.target.value)}
                            value={gatewayType}
                        >
                            {gatewayTypes.map((type) => (
                                <option key={type} value={type}>{type}</option>
                            ))}
                        </Select>
                    </Field>
                    <Field label="Agent" required>
                        <Select
                            onChange={(e) => setAgentId(e.target.value)}
                            required
                            value={agentId}
                        >
                            <option disabled value="">Select an agent</option>
                            {agents.map((agent) => (
                                <option key={agent.agent_id} value={agent.agent_id}>
                                    {agent.name} ({agent.agent_id})
                                </option>
                            ))}
                        </Select>
                    </Field>
                    <div className="span-2">
                        <Checkbox
                            checked={enabled}
                            label="Start automatically when AgentSpace boots"
                            onChange={(_, data) => setEnabled(data.checked === true)}
                        />
                    </div>
                    {schemaLoading && (
                        <span className="muted-sm span-2">Loading gateway schema…</span>
                    )}
                    {schema && schema.fields.length > 0
                        && renderSchemaFields(schema.fields, schemaValues, updateSchemaValue)}
                    <Field
                        className="span-2"
                        hint="One KEY=VALUE per line."
                        label="Other environment variables"
                    >
                        <Textarea
                            onChange={(e) => setEnvVars(e.target.value)}
                            placeholder="EXTRA_VAR=value"
                            rows={4}
                            value={envVars}
                        />
                    </Field>
                    {renderSecretRows(newSecrets, updateSecret, removeSecret, addSecret)}
                </div>
            </FormDialog>

            <FormDialog
                busy={busy || !editAgentId || !editSchema || agentsLoading
                    || !validAgentIds.has(editAgentId)}
                onOpenChange={(open) => {
                    if (!open) cancelEdit();
                }}
                onSubmit={() => {
                    void handleEditSubmit();
                }}
                open={editingGateway !== null}
                submitLabel="Save changes"
                title={`Edit ${editingGateway?.name ?? "gateway"}`}
            >
                <div className="form-grid">
                    {editingGateway?.status === "running" && (
                        <MessageBar className="span-2" intent="warning">
                            <MessageBarBody>
                                Saving tears down the running container and respawns it with the
                                new configuration.
                            </MessageBarBody>
                        </MessageBar>
                    )}
                    <Field label="Name" required>
                        <Input
                            onChange={(e) => patchEditDraft({ name: e.target.value })}
                            required
                            value={editName}
                        />
                    </Field>
                    <Field
                        label="Agent"
                        required
                        validationMessage={Boolean(editAgentId) && !agentsLoading
                                && !validAgentIds.has(editAgentId)
                            ? "The assigned agent no longer exists. Pick an existing agent."
                            : undefined}
                    >
                        <Select
                            onChange={(e) => patchEditDraft({ agentId: e.target.value })}
                            required
                            value={editAgentId}
                        >
                            <option disabled value="">Select an agent</option>
                            {Boolean(editAgentId) && !agentsLoading
                                && !validAgentIds.has(editAgentId) && (
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
                    </Field>
                    <div className="span-2">
                        <Checkbox
                            checked={editEnabled}
                            label="Start automatically when AgentSpace boots"
                            onChange={(_, data) => patchEditDraft({ enabled: data.checked === true })}
                        />
                    </div>
                    {editSchemaLoading && (
                        <span className="muted-sm span-2">Loading gateway schema…</span>
                    )}
                    {editSchema && editSchema.fields.length > 0 && renderSchemaFields(
                        editSchema.fields,
                        editSchemaValues,
                        updateEditSchemaValue,
                        editingGateway?.secret_keys ?? [],
                    )}
                    <Field
                        className="span-2"
                        hint="One KEY=VALUE per line."
                        label="Other environment variables"
                    >
                        <Textarea
                            onChange={(e) => patchEditDraft({ extraEnv: e.target.value })}
                            placeholder="EXTRA_VAR=value"
                            rows={4}
                            value={editExtraEnv}
                        />
                    </Field>
                    {renderSecretRows(
                        editNewSecrets,
                        updateEditSecret,
                        removeEditSecret,
                        addEditSecret,
                    )}
                </div>
            </FormDialog>

            <LogsDialog
                lines={logsQuery.data?.lines ?? []}
                loading={logsQuery.isFetching}
                onClose={() => setLogsGatewayId(null)}
                onRefresh={() => {
                    void logsQuery.refetch();
                }}
                open={logsGateway !== null}
                title={logsGateway ? `${logsGateway.name} logs` : "Logs"}
            />
        </div>
    );
}
