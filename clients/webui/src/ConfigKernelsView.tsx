import { useEffect, useState } from "react";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { withRequiredEnvKeys } from "./envPrefill";

type ConfigKernelsViewProps = {
    harnesses: string[];
    onError: (message: string) => void;
};

const CONFIGURABLE_HARNESSES = new Set(["opencode"]);

const OPENCODE_ENV_KEYS = [
    "OPENCODE_MODEL",
    "OPENCODE_VARIANT",
    "OPENCODE_AGENT",
    "OPENCODE_EXTRA_ARGS",
];

function formatHarnessLabel(harness: string): string {
    return harness
        .split("-")
        .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
        .join(" ");
}

export default function ConfigKernelsView({ harnesses, onError }: ConfigKernelsViewProps) {
    const [selected, setSelected] = useState<string | null>(null);
    const [envVars, setEnvVars] = useState("");
    const [loading, setLoading] = useState(false);
    const [saving, setSaving] = useState(false);
    const [updatedAt, setUpdatedAt] = useState<string | null>(null);
    const [dirty, setDirty] = useState(false);
    const [savedNotice, setSavedNotice] = useState(false);
    const [loadError, setLoadError] = useState<string | null>(null);

    // Fall back to the first harness until the user picks one explicitly.
    // Computed in render so we don't need an effect to assign a default.
    const effectiveSelected: string | null = selected ?? harnesses[0] ?? null;

    useEffect(() => {
        if (effectiveSelected === null) return;
        if (!CONFIGURABLE_HARNESSES.has(effectiveSelected)) {
            setEnvVars("");
            setUpdatedAt(null);
            setDirty(false);
            setLoadError(null);
            return;
        }
        setLoading(true);
        setSavedNotice(false);
        setLoadError(null);
        api.getKernelConfig(effectiveSelected)
            .then((config) => {
                setEnvVars(withRequiredEnvKeys(config.env_vars, effectiveSelected));
                setUpdatedAt(config.updated_at);
                setDirty(false);
            })
            .catch((err: Error) => {
                setLoadError(err.message);
                // Still surface required keys so the user knows what to fill
                // in even when the stored config can't be loaded.
                setEnvVars((prev) => withRequiredEnvKeys(prev, effectiveSelected));
            })
            .finally(() => setLoading(false));
    }, [effectiveSelected]);

    async function handleSave() {
        if (effectiveSelected === null) return;
        setSaving(true);
        setSavedNotice(false);
        try {
            const config = await api.updateKernelConfig(effectiveSelected, envVars);
            setEnvVars(withRequiredEnvKeys(config.env_vars, effectiveSelected));
            setUpdatedAt(config.updated_at);
            setDirty(false);
            setSavedNotice(true);
        } catch (err) {
            onError((err as Error).message);
        } finally {
            setSaving(false);
        }
    }

    return (
        <div className="view-content">
            <div className="view-header">
                <h2>Kernel Configuration</h2>
            </div>
            <p className="muted">
                These values act as defaults that pre-fill the Environment Variables
                field when creating a new agent.
            </p>

            <div className="config-kernels-layout">
                <aside className="config-kernels-list card">
                    <h3>Kernels</h3>
                    <ul className="plain-list">
                        {harnesses.map((harness) => (
                            <li key={harness}>
                                <button
                                    className={`list-item ${effectiveSelected === harness ? "active" : ""}`}
                                    onClick={() => setSelected(harness)}
                                    type="button"
                                >
                                    <span>{formatHarnessLabel(harness)}</span>
                                    {CONFIGURABLE_HARNESSES.has(harness) ? null : (
                                        <span className="tag muted-tag">WIP</span>
                                    )}
                                </button>
                            </li>
                        ))}
                        {harnesses.length === 0 && (
                            <li className="empty-state">No kernels available.</li>
                        )}
                    </ul>
                </aside>

                <section className="config-kernels-detail card">
                    {effectiveSelected === null && (
                        <div className="empty-state">Select a kernel.</div>
                    )}
                    {effectiveSelected !== null && !CONFIGURABLE_HARNESSES.has(effectiveSelected) && (
                        <div>
                            <h3>{formatHarnessLabel(effectiveSelected)}</h3>
                            <p className="muted">Configuration for this kernel is a work in progress.</p>
                        </div>
                    )}
                    {effectiveSelected !== null && CONFIGURABLE_HARNESSES.has(effectiveSelected) && (
                        <div>
                            <h3>{formatHarnessLabel(effectiveSelected)}</h3>
                            <p className="muted">
                                Recognized keys: {OPENCODE_ENV_KEYS.map((k) => (
                                    <code key={k} style={{ marginRight: 6 }}>{k}</code>
                                ))}
                            </p>
                            {loadError !== null && (
                                <p className="muted" style={{ color: "var(--danger, #c44)" }}>
                                    Failed to load: {loadError}
                                </p>
                            )}
                            <label>Environment Variables</label>
                            <CodeEditor
                                value={envVars}
                                onChange={(v) => { setEnvVars(v); setDirty(true); setSavedNotice(false); }}
                                language="ini"
                                height="200px"
                            />
                            <span className="muted">Use .env file syntax: KEY=VALUE, one per line</span>
                            <div className="form-actions" style={{ marginTop: 12, display: "flex", gap: 8, alignItems: "center" }}>
                                <button disabled={saving || loading || !dirty} onClick={() => { void handleSave(); }} type="button">
                                    {saving ? "Saving…" : "Save"}
                                </button>
                                {updatedAt !== null && (
                                    <span className="muted">
                                        Last saved {new Date(updatedAt).toLocaleString()}
                                    </span>
                                )}
                                {savedNotice && <span className="muted">Saved.</span>}
                            </div>
                        </div>
                    )}
                </section>
            </div>
        </div>
    );
}
