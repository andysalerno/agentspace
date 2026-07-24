import { useState } from "react";
import type { ChangeEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import CodeEditor from "./CodeEditor";
import { useErrorContext } from "./useErrorContext";
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

export default function ConfigurationView() {
  const [source, setSource] = useState(EMPTY_CONFIG);
  const [result, setResult] = useState<object | null>(null);
  const queryClient = useQueryClient();
  const { reportError } = useErrorContext();

  const mutation = useMutation({
    mutationFn: ({ action, yaml }: { action: ConfigAction; yaml: string }) => {
      if (action === "validate") return api.validateConfig(yaml);
      if (action === "plan") return api.planConfig(yaml);
      return api.applyConfig(yaml);
    },
    onSuccess: (value, variables) => {
      setResult(value);
      if (variables.action === "apply") {
        void queryClient.invalidateQueries();
      }
    },
    onError: reportError,
  });

  async function loadFile(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file) return;
    setSource(await file.text());
    setResult(null);
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
            accept=".yaml,.yml,text/yaml"
            onChange={(event) => { void loadFile(event); }}
            type="file"
          />
        </label>
        <CodeEditor
          height="480px"
          language="yaml"
          onChange={(value) => {
            setSource(value);
            setResult(null);
          }}
          value={source}
        />
        <div className="form-actions" style={{ marginTop: 12 }}>
          <Button
            className="secondary-button"
            disabled={mutation.isPending}
            onClick={() => mutation.mutate({ action: "validate", yaml: source })}
            type="button"
          >
            Validate
          </Button>
          <Button
            className="secondary-button"
            disabled={mutation.isPending}
            onClick={() => mutation.mutate({ action: "plan", yaml: source })}
            type="button"
          >
            Preview Replacement
          </Button>
          <Button
            disabled={mutation.isPending}
            onClick={() => mutation.mutate({ action: "apply", yaml: source })}
            type="button"
          >
            {mutation.isPending ? "Working…" : "Apply Replacement"}
          </Button>
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
