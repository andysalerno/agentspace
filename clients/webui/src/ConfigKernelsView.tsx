import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ArrowDownload20Regular } from "@fluentui/react-icons";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { withRequiredEnvKeys } from "./envPrefill";
import { queryKeys, useHarnesses, useKernelConfig } from "./queries";
import { useErrorContext } from "./useErrorContext";
import { Button, Field, MessageBar, MessageBarBody } from "./fluent";
import { formatHarnessLabel } from "./harness";
import { ViewHeader } from "./ui";

const CONFIGURABLE_HARNESSES = new Set(["opencode"]);

const OPENCODE_ENV_KEYS = [
    "OPENCODE_MODEL",
    "OPENCODE_VARIANT",
    "OPENCODE_AGENT",
    "OPENCODE_EXTRA_ARGS",
];

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
    const isConfigurable = effectiveSelected !== null
        && CONFIGURABLE_HARNESSES.has(effectiveSelected);

    const configQuery = useKernelConfig(isConfigurable ? effectiveSelected : null);

    const serverEnvVars = isConfigurable && effectiveSelected !== null && configQuery.data
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
        <div className="view-content">
            <ViewHeader
                description="Defaults that pre-fill the environment variables field when creating an agent."
                title="Kernel configuration"
            />
            <div className="view-body">
                <div className="split-layout">
                    <div className="panel">
                        <ul className="nav-list">
                            {harnesses.map((harness) => (
                                <li key={harness}>
                                    <button
                                        aria-current={effectiveSelected === harness}
                                        className={`list-item${
                                            effectiveSelected === harness ? " active" : ""
                                        }`}
                                        onClick={() => selectHarness(harness)}
                                        type="button"
                                    >
                                        <span>{formatHarnessLabel(harness)}</span>
                                        {!CONFIGURABLE_HARNESSES.has(harness) && (
                                            <span className="tag">Planned</span>
                                        )}
                                    </button>
                                </li>
                            ))}
                            {harnesses.length === 0 && (
                                <li className="muted-sm" style={{ padding: "var(--space-2)" }}>
                                    No kernels available.
                                </li>
                            )}
                        </ul>
                    </div>

                    <div className="panel">
                        <div className="panel-header">
                            <h3>
                                {effectiveSelected === null
                                    ? "No kernel selected"
                                    : formatHarnessLabel(effectiveSelected)}
                            </h3>
                            {effectiveSelected !== null && isConfigurable && (
                                <div className="view-header-actions">
                                    {savedNotice && <span className="muted-sm">Saved</span>}
                                    {!savedNotice && updatedAt !== null && (
                                        <span className="muted-sm">
                                            Last saved {new Date(updatedAt).toLocaleString()}
                                        </span>
                                    )}
                                    <Button
                                        icon={<ArrowDownload20Regular />}
                                        onClick={() => {
                                            void api.downloadConfigResource(
                                                "kernel-config",
                                                effectiveSelected,
                                            ).catch(reportError);
                                        }}
                                        size="small"
                                    >
                                        Export YAML
                                    </Button>
                                    <Button
                                        appearance="primary"
                                        disabled={saveMutation.isPending || loading || !dirty}
                                        onClick={handleSave}
                                        size="small"
                                    >
                                        {saveMutation.isPending ? "Saving…" : "Save"}
                                    </Button>
                                </div>
                            )}
                        </div>
                        <div className="panel-body">
                            {effectiveSelected !== null && !isConfigurable && (
                                <p className="muted">
                                    Configuration for this kernel is not implemented yet.
                                </p>
                            )}
                            {effectiveSelected !== null && isConfigurable && (
                                <>
                                    {loadError !== null && (
                                        <MessageBar intent="error">
                                            <MessageBarBody>
                                                Failed to load configuration:{" "}
                                                {loadError instanceof Error
                                                    ? loadError.message
                                                    : String(loadError)}
                                            </MessageBarBody>
                                        </MessageBar>
                                    )}
                                    <Field
                                        hint="One KEY=VALUE per line, using .env syntax."
                                        label="Environment variables"
                                    >
                                        <CodeEditor
                                            ariaLabel="Environment variables"
                                            height="240px"
                                            language="ini"
                                            onChange={handleEditorChange}
                                            value={envVars}
                                        />
                                    </Field>
                                    <p className="muted-sm">
                                        Recognised keys:{" "}
                                        <span className="tag-row">
                                            {OPENCODE_ENV_KEYS.map((key) => (
                                                <span className="tag mono" key={key}>{key}</span>
                                            ))}
                                        </span>
                                    </p>
                                </>
                            )}
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}
