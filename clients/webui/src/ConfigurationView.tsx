import { useState } from "react";
import type { ChangeEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { useCanonicalConfig } from "./queries";
import { useErrorContext } from "./useErrorContext";
import { Button } from "./fluent";

const LOADING_CONFIG = "# Loading current configuration…\n";

type ConfigAction = "validate" | "plan" | "apply";
type ConfigInput = string | Blob;

export default function ConfigurationView() {
  const [draft, setDraft] = useState<string | null>(null);
  const [bundle, setBundle] = useState<File | null>(null);
  const [result, setResult] = useState<object | null>(null);
  const [expectedGeneration, setExpectedGeneration] = useState<number | null>(null);
  const queryClient = useQueryClient();
  const { reportError } = useErrorContext();
  const canonicalQuery = useCanonicalConfig();

  // The editor shows the live canonical configuration until the user edits it
  // or loads a file; from then on the local draft wins until it is reverted.
  const source = draft ?? canonicalQuery.data ?? LOADING_CONFIG;

  const mutation = useMutation({
    mutationFn: ({
      action,
      input,
      generation,
    }: {
      action: ConfigAction;
      input: ConfigInput;
      generation?: number;
    }) => {
      if (action === "validate") return api.validateConfig(input);
      if (action === "plan") return api.planConfig(input);
      return api.applyConfig(input, generation);
    },
    onSuccess: (value, variables) => {
      setResult(value);
      if (
        variables.action === "plan"
        && typeof value.active_generation === "number"
      ) {
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

  return (
    <div className="view-content management-view">
      <div className="view-header">
        <div>
          <h2>Declarative Configuration</h2>
          <span className="muted">
            Validate, preview, replace, and export the complete configuration.
          </span>
        </div>
        <div className="view-header-actions">
          <Button
            className="secondary-button"
            onClick={() => download("source")}
            type="button"
          >
            Export Source
          </Button>
          <Button
            className="secondary-button"
            onClick={() => download("canonical")}
            type="button"
          >
            Export Canonical YAML
          </Button>
        </div>
      </div>

      <div className="card management-card">
        <p className="muted">
          The editor is loaded with the canonical YAML for the configuration that
          is active right now. Applying this document atomically replaces all
          in-scope configuration. Workspaces, sessions, and secret values are not
          changed.
        </p>
        <label>
          Load YAML
          <input
            accept=".yaml,.yml,.zip,text/yaml,application/zip"
            onChange={(event) => { void loadFile(event); }}
            type="file"
          />
        </label>
        {bundle !== null && (
          <p className="muted">
            Loaded config-set bundle: <code>{bundle.name}</code>
          </p>
        )}
        {(draft !== null || bundle !== null) && (
          <p className="muted">
            Showing local edits instead of the active configuration.{" "}
            <Button
              className="secondary-button"
              onClick={revert}
              type="button"
            >
              Discard And Reload
            </Button>
          </p>
        )}
        <CodeEditor
          height="480px"
          language="yaml"
          onChange={(value) => {
            setDraft(value);
            setBundle(null);
            setResult(null);
            setExpectedGeneration(null);
          }}
          value={source}
        />
        <div className="form-actions" style={{ marginTop: 12 }}>
          <Button
            className="secondary-button"
            disabled={busy}
            onClick={() => mutation.mutate({ action: "validate", input: bundle ?? source })}
            type="button"
          >
            Validate
          </Button>
          <Button
            className="secondary-button"
            disabled={busy}
            onClick={() => mutation.mutate({ action: "plan", input: bundle ?? source })}
            type="button"
          >
            Preview Replacement
          </Button>
          <Button
            disabled={busy}
            onClick={() => mutation.mutate({
              action: "apply",
              input: bundle ?? source,
              generation: expectedGeneration ?? undefined,
            })}
            type="button"
          >
            {mutation.isPending ? "Working…" : "Apply Replacement"}
          </Button>
          {expectedGeneration !== null && (
            <span className="muted">
              Applying against generation {expectedGeneration}
            </span>
          )}
        </div>
      </div>

      {result !== null && (
        <div className="card management-card">
          <h3>Result</h3>
          <pre className="config-result">{JSON.stringify(result, null, 2)}</pre>
        </div>
      )}
    </div>
  );
}
