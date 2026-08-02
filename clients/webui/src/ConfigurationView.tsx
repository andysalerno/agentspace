import { useRef, useState } from "react";
import type { ChangeEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
    ArrowDownload20Regular,
    ArrowUndo20Regular,
    CheckmarkCircle20Regular,
    DocumentArrowUp20Regular,
    Eye20Regular,
} from "@fluentui/react-icons";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { useCanonicalConfig } from "./queries";
import { useErrorContext } from "./useErrorContext";
import {
    Button,
    Menu,
    MenuItem,
    MenuList,
    MenuPopover,
    MenuTrigger,
    MessageBar,
    MessageBarBody,
    Toolbar,
    ToolbarButton,
    ToolbarDivider,
} from "./fluent";
import { ViewHeader } from "./ui";

const LOADING_CONFIG = "# Loading current configuration…\n";

type ConfigAction = "validate" | "plan" | "apply";
type ConfigInput = string | Blob;

export default function ConfigurationView() {
    const [draft, setDraft] = useState<string | null>(null);
    const [bundle, setBundle] = useState<File | null>(null);
    const [result, setResult] = useState<object | null>(null);
    const [expectedGeneration, setExpectedGeneration] = useState<number | null>(null);
    const fileInput = useRef<HTMLInputElement>(null);
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();
    const canonicalQuery = useCanonicalConfig();

    // The editor shows the live canonical configuration until the user edits it
    // or loads a file; from then on the local draft wins until it is reverted.
    const source = draft ?? canonicalQuery.data ?? LOADING_CONFIG;

    const mutation = useMutation({
        mutationFn: (
            { action, input, generation }: {
                action: ConfigAction;
                input: ConfigInput;
                generation?: number;
            },
        ) => {
            if (action === "validate") return api.validateConfig(input);
            if (action === "plan") return api.planConfig(input);
            return api.applyConfig(input, generation);
        },
        onSuccess: (value, variables) => {
            setResult(value);
            if (variables.action === "plan" && typeof value.active_generation === "number") {
                setExpectedGeneration(value.active_generation);
            }
            if (variables.action === "apply") {
                setExpectedGeneration(null);
                setDraft(null);
                setBundle(null);
                void queryClient.invalidateQueries();
            }
        },
        onError: reportError,
    });

    async function loadFile(event: ChangeEvent<HTMLInputElement>) {
        const file = event.target.files?.[0];
        if (!file) return;
        if (file.name.toLowerCase().endsWith(".zip")) {
            setBundle(file);
        } else {
            setDraft(await file.text());
            setBundle(null);
        }
        setResult(null);
        setExpectedGeneration(null);
        event.target.value = "";
    }

    function revert() {
        setDraft(null);
        setBundle(null);
        setResult(null);
        setExpectedGeneration(null);
        void canonicalQuery.refetch();
    }

    function download(mode: "source" | "canonical") {
        void api.downloadConfig(mode).catch(reportError);
    }

    const busy = mutation.isPending || canonicalQuery.isPending;
    const dirty = draft !== null || bundle !== null;

    return (
        <div className="view-content">
            <ViewHeader
                actions={
                    <Menu positioning="below-end">
                        <MenuTrigger disableButtonEnhancement>
                            <Button icon={<ArrowDownload20Regular />}>Export</Button>
                        </MenuTrigger>
                        <MenuPopover>
                            <MenuList>
                                <MenuItem onClick={() => download("source")}>
                                    Source as authored
                                </MenuItem>
                                <MenuItem onClick={() => download("canonical")}>
                                    Canonical YAML
                                </MenuItem>
                            </MenuList>
                        </MenuPopover>
                    </Menu>
                }
                description="Validate, preview, and replace the complete declarative configuration."
                title="Configuration"
            />
            <div className="view-body">
                <MessageBar intent="info">
                    <MessageBarBody>
                        Applying atomically replaces all in-scope configuration. Workspaces,
                        sessions, and secret values are left unchanged.
                    </MessageBarBody>
                </MessageBar>

                {dirty && (
                    <MessageBar intent="warning">
                        <MessageBarBody>
                            {bundle !== null
                                ? `Showing the loaded bundle ${bundle.name} instead of the active configuration.`
                                : "Showing local edits instead of the active configuration."}
                        </MessageBarBody>
                    </MessageBar>
                )}

                <div className="panel">
                    <div className="panel-header">
                        <Toolbar aria-label="Configuration actions" size="small">
                            <ToolbarButton
                                icon={<DocumentArrowUp20Regular />}
                                onClick={() => fileInput.current?.click()}
                            >
                                Load file
                            </ToolbarButton>
                            <ToolbarButton
                                disabled={!dirty}
                                icon={<ArrowUndo20Regular />}
                                onClick={revert}
                            >
                                Discard edits
                            </ToolbarButton>
                            <ToolbarDivider />
                            <ToolbarButton
                                disabled={busy}
                                icon={<CheckmarkCircle20Regular />}
                                onClick={() =>
                                    mutation.mutate({
                                        action: "validate",
                                        input: bundle ?? source,
                                    })}
                            >
                                Validate
                            </ToolbarButton>
                            <ToolbarButton
                                disabled={busy}
                                icon={<Eye20Regular />}
                                onClick={() =>
                                    mutation.mutate({ action: "plan", input: bundle ?? source })}
                            >
                                Preview replacement
                            </ToolbarButton>
                        </Toolbar>
                        <div className="view-header-actions">
                            {expectedGeneration !== null && (
                                <span className="muted-sm">
                                    Applying against generation {expectedGeneration}
                                </span>
                            )}
                            <Button
                                appearance="primary"
                                disabled={busy}
                                onClick={() =>
                                    mutation.mutate({
                                        action: "apply",
                                        input: bundle ?? source,
                                        generation: expectedGeneration ?? undefined,
                                    })}
                            >
                                {mutation.isPending ? "Working…" : "Apply replacement"}
                            </Button>
                        </div>
                    </div>
                    {/* Opened by the "Load file" toolbar button; kept out of the
                        tab order so it is not an invisible extra stop. */}
                    <input
                        accept=".yaml,.yml,.zip,text/yaml,application/zip"
                        aria-hidden="true"
                        className="visually-hidden"
                        onChange={(event) => {
                            void loadFile(event);
                        }}
                        ref={fileInput}
                        tabIndex={-1}
                        type="file"
                    />
                    <CodeEditor
                        ariaLabel="Declarative configuration"
                        height="calc(100vh - 420px)"
                        language="yaml"
                        onChange={(value) => {
                            setDraft(value);
                            setBundle(null);
                            setResult(null);
                            setExpectedGeneration(null);
                        }}
                        value={source}
                    />
                </div>

                {result !== null && (
                    <div className="panel">
                        <div className="panel-header">
                            <h3>Result</h3>
                        </div>
                        <div className="panel-body">
                            <pre className="code-block">{JSON.stringify(result, null, 2)}</pre>
                        </div>
                    </div>
                )}
            </div>
        </div>
    );
}
