import { useEffect, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { withRequiredEnvKeys } from "./envPrefill";
import { queryKeys, useHarnesses, useKernelConfig } from "./queries";
import { useErrorContext } from "./ErrorContext";
import { Button } from "./fluent";

const CONFIGURABLE_HARNESSES = new Set(["acp"]);

const ACP_ENV_KEYS = [
    "KERNEL_ACP_SERVER",
    "KERNEL_ACP_MODEL_NAME",
    "KERNEL_ACP_COPILOT_EXPERIMENTAL_ENABLED",
    "KERNEL_ACP_COMMAND",
    "KERNEL_ACP_EXTRA_ARGS",
];

function formatHarnessLabel(harness: string): string {
    return harness
        .split("-")
        .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
        .join(" ");
}

export default function ConfigKernelsView() {
    const { data: harnesses = [] } = useHarnesses();
    const { reportError } = useErrorContext();
    const queryClient = useQueryClient();

    const [selected, setSelected] = useState<string | null>(null);
    const [envVars, setEnvVars] = useState("");
    const [dirty, setDirty] = useState(false);
    const [savedNotice, setSavedNotice] = useState(false);

    // Fall back to the first harness until the user picks one explicitly.
    const effectiveSelected: string | null = selected ?? harnesses[0] ?? null;
    const isConfigurable =
        effectiveSelected !== null && CONFIGURABLE_HARNESSES.has(effectiveSelected);

    const configQuery = useKernelConfig(isConfigurable ? effectiveSelected : null);

    // Track which server payload (keyed by harness + updated_at) has been
    // copied into the editor. This prevents a stale `configQuery.data`
    // (still in cache before the post-save refetch lands) from clobbering
    // the freshly-saved value the mutation just wrote into local state.
    const appliedRef = useRef<string | null>(null);

    // Sync server config into local editor state when it changes (and we're
    // not mid-edit). The editor needs its own state for the dirty/saved
    // tracking.
    useEffect(() => {
        if (!isConfigurable || effectiveSelected === null) {
            setEnvVars("");
            setDirty(false);
            appliedRef.current = null;
            return;
        }
        const data = configQuery.data;
        if (!data || dirty) return;
        const stamp = `${effectiveSelected}::${data.updated_at}`;
        if (appliedRef.current === stamp) return;
        appliedRef.current = stamp;
        setEnvVars(withRequiredEnvKeys(data.env_vars, effectiveSelected));
    }, [configQuery.data, isConfigurable, effectiveSelected, dirty]);

    // When the selected harness changes, clear dirty/saved state.
    useEffect(() => {
        setDirty(false);
        setSavedNotice(false);
    }, [effectiveSelected]);

    const saveMutation = useMutation({
        mutationFn: ({ harness, value }: { harness: string; value: string }) =>
            api.updateKernelConfig(harness, value),
        onSuccess: (config, variables) => {
            setEnvVars(withRequiredEnvKeys(config.env_vars, variables.harness));
            setDirty(false);
            setSavedNotice(true);
            // Mark this payload as already applied so the sync effect won't
            // re-copy the (still-stale) cached value back into the editor
            // before the invalidation refetch completes.
            appliedRef.current = `${variables.harness}::${config.updated_at}`;
            void queryClient.invalidateQueries({
                queryKey: queryKeys.kernelConfig(variables.harness),
            });
        },
        onError: reportError,
    });

    function handleSave() {
        if (effectiveSelected === null) return;
        setSavedNotice(false);
        saveMutation.mutate({ harness: effectiveSelected, value: envVars });
    }

    const updatedAt = configQuery.data?.updated_at ?? null;
    const loadError = configQuery.error;
    const loading = configQuery.isFetching;

    return (
        <div className="view-content management-view config-kernels-management-view">
            <div className="view-header">
                <div>
                    <h2>Kernel Configuration</h2>
                    <span className="muted">
                        {harnesses.length} kernels · {CONFIGURABLE_HARNESSES.size} configurable
                    </span>
                </div>
            </div>
            <p className="muted management-intro">
                These values act as defaults that pre-fill the Environment Variables
                field when creating a new agent.
            </p>

            <div className="config-kernels-layout">
                <div className="config-kernels-list card management-card">
                    <h3>Kernels</h3>
                    <ul className="plain-list">
                        {harnesses.map((harness) => (
                            <li key={harness}>
                                <Button
                                    className={`list-item ${effectiveSelected === harness ? "active" : ""}`}
                                    onClick={() => setSelected(harness)}
                                    type="button"
                                >
                                    <span>{formatHarnessLabel(harness)}</span>
                                    {CONFIGURABLE_HARNESSES.has(harness) ? null : (
                                        <span className="tag muted-tag">WIP</span>
                                    )}
                                </Button>
                            </li>
                        ))}
                        {harnesses.length === 0 && (
                            <li className="empty-state">No kernels available.</li>
                        )}
                    </ul>
                </div>

                <div className="config-kernels-detail card management-card">
                    {effectiveSelected === null && (
                        <div className="empty-state">Select a kernel.</div>
                    )}
                    {effectiveSelected !== null && !isConfigurable && (
                        <div>
                            <h3>{formatHarnessLabel(effectiveSelected)}</h3>
                            <p className="muted">Configuration for this kernel is a work in progress.</p>
                        </div>
                    )}
                    {effectiveSelected !== null && isConfigurable && (
                        <div>
                            <h3>{formatHarnessLabel(effectiveSelected)}</h3>
                            <p className="muted">
                                Recognized keys: {ACP_ENV_KEYS.map((k) => (
                                    <code key={k} style={{ marginRight: 6 }}>{k}</code>
                                ))}
                            </p>
                            {loadError !== null && (
                                <p className="muted" style={{ color: "var(--danger, #c44)" }}>
                                    Failed to load:{" "}
                                    {loadError instanceof Error ? loadError.message : String(loadError)}
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
                                <Button
                                    disabled={saveMutation.isPending || loading || !dirty}
                                    onClick={handleSave}
                                    type="button"
                                >
                                    {saveMutation.isPending ? "Saving…" : "Save"}
                                </Button>
                                {updatedAt !== null && (
                                    <span className="muted">
                                        Last saved {new Date(updatedAt).toLocaleString()}
                                    </span>
                                )}
                                {savedNotice && <span className="muted">Saved.</span>}
                            </div>
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
}
