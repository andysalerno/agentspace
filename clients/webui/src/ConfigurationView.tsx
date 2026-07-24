import { useState } from "react";
import type { ChangeEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { useErrorContext } from "./ErrorContext";
import { Button } from "./fluent";

const EMPTY_CONFIG = `apiVersion: agentspace.dev/v1alpha1
kind: AgentSpaceConfig
metadata:
  name: local
spec:
  secrets: []
  kernelConfigs: []
  connections: []
  skills: []
  agents: []
  gateways: []
`;

type ConfigAction = "validate" | "plan" | "apply";
type ConfigInput = string | Blob;

export default function ConfigurationView() {
  const [source, setSource] = useState(EMPTY_CONFIG);
  const [bundle, setBundle] = useState<File | null>(null);
  const [result, setResult] = useState<object | null>(null);
  const [expectedGeneration, setExpectedGeneration] = useState<number | null>(null);
  const queryClient = useQueryClient();
  const { reportError } = useErrorContext();

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
      setSource(await file.text());
      setBundle(null);
    }
    setResult(null);
    setExpectedGeneration(null);
  }

  function download(mode: "source" | "canonical") {
    void api.downloadConfig(mode).catch(reportError);
  }

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
          Applying this document atomically replaces all in-scope configuration.
          Workspaces, sessions, and secret values are not changed.
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
        <CodeEditor
          height="480px"
          language="yaml"
          onChange={(value) => {
            setSource(value);
            setBundle(null);
            setResult(null);
            setExpectedGeneration(null);
          }}
          value={source}
        />
        <div className="form-actions" style={{ marginTop: 12 }}>
          <Button
            className="secondary-button"
            disabled={mutation.isPending}
            onClick={() => mutation.mutate({ action: "validate", input: bundle ?? source })}
            type="button"
          >
            Validate
          </Button>
          <Button
            className="secondary-button"
            disabled={mutation.isPending}
            onClick={() => mutation.mutate({ action: "plan", input: bundle ?? source })}
            type="button"
          >
            Preview Replacement
          </Button>
          <Button
            disabled={mutation.isPending}
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
