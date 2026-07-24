import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { withRequiredEnvKeys } from "./envPrefill";
import { queryKeys, useHarnesses, useKernelConfig } from "./queries";
import { useErrorContext } from "./useErrorContext";
import { Button } from "./fluent";

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

export default function ConfigKernelsView() {
    const { data: harnesses = [] } = useHarnesses();
    const { reportError } = useErrorContext();
    const queryClient = useQueryClient();

    const [selected, setSelected] = useState<string | null>(null);
    // Local editor edits, scoped to the harness they were made against. `null`
    // means "not edited" and the editor shows the server value.
    const [draft, setDraft] = useState<{ harness: string; value: string } | null>(null);
    const [savedNoticeFor, setSavedNoticeFor] = useState<string | null>(null);

    // Fall back to the first harness until the user picks one explicitly.
    const effectiveSelected: string | null = selected ?? harnesses[0] ?? null;
    const isConfigurable =
        effectiveSelected !== null && CONFIGURABLE_HARNESSES.has(effectiveSelected);

    const configQuery = useKernelConfig(isConfigurable ? effectiveSelected : null);

    const serverEnvVars =
        isConfigurable && effectiveSelected !== null && configQuery.data
            ? withRequiredEnvKeys(configQuery.data.env_vars, effectiveSelected)
            : "";
    const dirty = draft !== null && draft.harness === effectiveSelected;
    const envVars = dirty ? draft.value : serverEnvVars;
    const savedNotice = savedNoticeFor !== null && savedNoticeFor === effectiveSelected;

    function selectHarness(harness: string) {
        setSelected(harness);
        setDraft(null);
        setSavedNoticeFor(null);
    }

    function handleEditorChange(value: string) {
        if (effectiveSelected === null) return;
        setDraft({ harness: effectiveSelected, value });
        setSavedNoticeFor(null);
    }

    const saveMutation = useMutation({
        mutationFn: ({ harness, value }: { harness: string; value: string }) =>
            api.updateKernelConfig(harness, value),
        onSuccess: (config, variables) => {
            // Write the server's response straight into the cache so the editor
            // never falls back to the stale pre-save value while the
            // invalidation refetch is in flight.
            queryClient.setQueryData(queryKeys.kernelConfig(variables.harness), config);
            setDraft(null);
            setSavedNoticeFor(variables.harness);
            void queryClient.invalidateQueries({
                queryKey: queryKeys.kernelConfig(variables.harness),
            });
        },
        onError: reportError,
    });

    function handleSave() {
        if (effectiveSelected === null) return;
        setSavedNoticeFor(null);
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
                                    onClick={() => selectHarness(harness)}
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
                                Recognized keys: {OPENCODE_ENV_KEYS.map((k) => (
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
                                onChange={handleEditorChange}
                                language="ini"
                                height="200px"
                            />
                            <span className="muted">Use .env file syntax: KEY=VALUE, one per line</span>
                            <div className="form-actions" style={{ marginTop: 12, display: "flex", gap: 8, alignItems: "center" }}>
                                <Button
                                    className="secondary-button"
                                    onClick={() => {
                                        void api.downloadConfigResource(
                                            "kernel-config",
                                            effectiveSelected,
                                        ).catch(reportError);
                                    }}
                                    type="button"
                                >
                                    Export YAML
                                </Button>
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
