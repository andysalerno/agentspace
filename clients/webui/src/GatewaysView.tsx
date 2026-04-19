import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { api } from "./api";
import type { Agent, Gateway, GatewayConfigField, GatewaySchema } from "./types";

type SecretEntry = { key: string; value: string };

type GatewaysViewProps = {
    gateways: Gateway[];
    agents: Agent[];
    gatewayTypes: string[];
    onCreateGateway: (payload: {
        gateway_id: string;
        name: string;
        gateway_type: string;
        agent_id: string;
        enabled: boolean;
        env_vars: string;
        secrets: Record<string, string>;
    }) => Promise<void>;
    onUpdateGateway: (
        gatewayId: string,
        payload: {
            name?: string;
            agent_id?: string;
            enabled?: boolean;
            env_vars?: string;
            secrets?: Record<string, string>;
        },
    ) => Promise<void>;
    onDeleteGateway: (gatewayId: string) => Promise<void>;
    onStartGateway: (gatewayId: string) => Promise<void>;
    onStopGateway: (gatewayId: string) => Promise<void>;
    busy: boolean;
};

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
    for (const field of schemaFields) {
        if (field.kind !== "env") continue;
        const value = schemaValues[field.key]?.trim();
        if (value) {
            lines.push(`${field.key}=${value}`);
        }
    }
    const trimmedExtra = extraEnv.trim();
    if (trimmedExtra) {
        lines.push(trimmedExtra);
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

export default function GatewaysView({
    gateways,
    agents,
    gatewayTypes,
    onCreateGateway,
    onUpdateGateway,
    onDeleteGateway,
    onStartGateway,
    onStopGateway,
    busy,
}: GatewaysViewProps) {
    const [showForm, setShowForm] = useState(false);
    const [gatewayId, setGatewayId] = useState("");
    const [gatewayName, setGatewayName] = useState("");
    const [gatewayType, setGatewayType] = useState(gatewayTypes[0] ?? "echo");
    const [agentId, setAgentId] = useState(agents[0]?.agent_id ?? "");
    const [enabled, setEnabled] = useState(false);
    const [envVars, setEnvVars] = useState("");
    const [newSecrets, setNewSecrets] = useState<SecretEntry[]>([]);
    const [schema, setSchema] = useState<GatewaySchema | null>(null);
    const [schemaLoading, setSchemaLoading] = useState(false);
    const [schemaValues, setSchemaValues] = useState<Record<string, string>>({});

    const [expandedGatewayId, setExpandedGatewayId] = useState<string | null>(null);
    const [logs, setLogs] = useState<string[] | null>(null);
    const [logsLoading, setLogsLoading] = useState(false);

    useEffect(() => {
        if (gatewayTypes.length > 0 && !gatewayTypes.includes(gatewayType)) {
            setGatewayType(gatewayTypes[0]);
        }
    }, [gatewayTypes, gatewayType]);

    useEffect(() => {
        if (!gatewayType) {
            setSchema(null);
            setSchemaValues({});
            return;
        }
        let cancelled = false;
        setSchemaLoading(true);
        api.getGatewayTypeSchema(gatewayType)
            .then((result) => {
                if (cancelled) return;
                setSchema(result);
                const initial: Record<string, string> = {};
                for (const field of result.fields) {
                    initial[field.key] = field.default ?? "";
                }
                setSchemaValues(initial);
            })
            .catch(() => {
                if (cancelled) return;
                setSchema({ fields: [] });
                setSchemaValues({});
            })
            .finally(() => {
                if (!cancelled) setSchemaLoading(false);
            });
        return () => {
            cancelled = true;
        };
    }, [gatewayType]);

    useEffect(() => {
        if (agents.length > 0 && !agents.some((a) => a.agent_id === agentId)) {
            setAgentId(agents[0].agent_id);
        }
    }, [agents, agentId]);

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
        await onCreateGateway({
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
        setSchemaValues(() => {
            const reset: Record<string, string> = {};
            for (const f of fields) reset[f.key] = f.default ?? "";
            return reset;
        });
        setShowForm(false);
    }

    function updateSchemaValue(key: string, value: string) {
        setSchemaValues((prev) => ({ ...prev, [key]: value }));
    }

    async function handleToggleLogs(gateway: Gateway) {
        if (expandedGatewayId === gateway.gateway_id) {
            setExpandedGatewayId(null);
            setLogs(null);
            return;
        }
        setLogsLoading(true);
        try {
            const result = await api.gatewayLogs(gateway.gateway_id);
            setLogs(result.lines);
            setExpandedGatewayId(gateway.gateway_id);
        } finally {
            setLogsLoading(false);
        }
    }

    return (
        <div className="view-content">
            <div className="view-header">
                <h2>Gateways</h2>
                <button onClick={() => setShowForm(!showForm)} type="button">
                    {showForm ? "Cancel" : "New Gateway"}
                </button>
            </div>

            {showForm && (
                <form className="create-form card" onSubmit={handleSubmit}>
                    <label>
                        Gateway ID
                        <input
                            pattern="[a-z]+(?:-[a-z]+)*"
                            placeholder="echo-bridge"
                            required
                            value={gatewayId}
                            onChange={(e) => setGatewayId(e.target.value)}
                        />
                    </label>
                    <label>
                        Name
                        <input
                            placeholder="My Echo Gateway"
                            required
                            value={gatewayName}
                            onChange={(e) => setGatewayName(e.target.value)}
                        />
                    </label>
                    <label>
                        Type
                        <select
                            value={gatewayType}
                            onChange={(e) => setGatewayType(e.target.value)}
                        >
                            {gatewayTypes.map((type) => (
                                <option key={type} value={type}>
                                    {type}
                                </option>
                            ))}
                        </select>
                    </label>
                    <label>
                        Agent
                        <select
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
                        </select>
                    </label>
                    <label className="checkbox-label">
                        <input
                            checked={enabled}
                            onChange={(e) => setEnabled(e.target.checked)}
                            type="checkbox"
                        />
                        Auto-start on boot
                    </label>
                    {schema && schema.fields.length > 0 && (
                        <fieldset className="schema-fields">
                            <legend>Gateway environment variables</legend>
                            {schema.fields.map((f) => (
                                <label key={f.key}>
                                    {f.label}
                                    {f.required && <span aria-hidden="true"> *</span>}
                                    <input
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
                        <textarea
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
                            <button
                                className="secondary-button small"
                                onClick={addSecret}
                                type="button"
                            >
                                + Add Secret
                            </button>
                        </div>
                        {newSecrets.map((secret, index) => (
                            <div className="skill-file-entry-header" key={index}>
                                <input
                                    placeholder="KEY"
                                    value={secret.key}
                                    onChange={(e) => updateSecret(index, "key", e.target.value)}
                                />
                                <input
                                    placeholder="value"
                                    type="password"
                                    value={secret.value}
                                    onChange={(e) => updateSecret(index, "value", e.target.value)}
                                />
                                <button
                                    className="icon-button danger-button"
                                    onClick={() => removeSecret(index)}
                                    type="button"
                                    title="Remove secret"
                                >
                                    ×
                                </button>
                            </div>
                        ))}
                    </div>
                    <button disabled={busy || !agentId} type="submit">
                        Create Gateway
                    </button>
                </form>
            )}

            <div className="card-grid">
                {gateways.map((gateway) => (
                    <div className="card" key={gateway.gateway_id}>
                        <div className="card-body">
                            <h3>
                                {gateway.name}
                                <span className={`badge status-${gateway.status}`}>
                                    {gateway.status}
                                </span>
                            </h3>
                            <div className="card-meta">
                                <div>
                                    <strong>ID:</strong> {gateway.gateway_id}
                                </div>
                                <div>
                                    <strong>Type:</strong> {gateway.gateway_type}
                                </div>
                                <div>
                                    <strong>Agent:</strong> {gateway.agent_id}
                                </div>
                                <div>
                                    <strong>Enabled:</strong>{" "}
                                    {gateway.enabled ? "yes" : "no"}
                                </div>
                                {gateway.container_name && (
                                    <div>
                                        <strong>Container:</strong> {gateway.container_name}
                                    </div>
                                )}
                                {gateway.secret_keys.length > 0 && (
                                    <div>
                                        <strong>Secrets:</strong>{" "}
                                        {gateway.secret_keys.join(", ")}
                                    </div>
                                )}
                                {gateway.last_error && (
                                    <div className="error-text">
                                        <strong>Error:</strong> {gateway.last_error}
                                    </div>
                                )}
                            </div>
                            {expandedGatewayId === gateway.gateway_id && logs && (
                                <pre className="skill-file-content">{logs.join("\n")}</pre>
                            )}
                        </div>
                        <div className="card-footer">
                            <button
                                className="secondary-button small"
                                disabled={logsLoading}
                                onClick={() => handleToggleLogs(gateway)}
                                type="button"
                            >
                                {expandedGatewayId === gateway.gateway_id
                                    ? "Hide Logs"
                                    : "View Logs"}
                            </button>
                            <div className="card-footer-actions">
                                {gateway.status === "running" ? (
                                    <button
                                        className="secondary-button small"
                                        disabled={busy}
                                        onClick={() => onStopGateway(gateway.gateway_id)}
                                        type="button"
                                    >
                                        Stop
                                    </button>
                                ) : (
                                    <button
                                        className="small"
                                        disabled={busy}
                                        onClick={() => onStartGateway(gateway.gateway_id)}
                                        type="button"
                                    >
                                        Start
                                    </button>
                                )}
                                <button
                                    className="secondary-button small"
                                    disabled={busy}
                                    onClick={() =>
                                        onUpdateGateway(gateway.gateway_id, {
                                            enabled: !gateway.enabled,
                                        })
                                    }
                                    type="button"
                                >
                                    {gateway.enabled ? "Disable" : "Enable"}
                                </button>
                                <button
                                    className="danger-button small"
                                    disabled={busy}
                                    onClick={() => onDeleteGateway(gateway.gateway_id)}
                                    type="button"
                                >
                                    Delete
                                </button>
                            </div>
                        </div>
                    </div>
                ))}
                {gateways.length === 0 && (
                    <div className="empty-state">
                        No gateways yet. Create one to bridge an external system to an agent.
                    </div>
                )}
            </div>
        </div>
    );
}
