import { useState } from "react";
import type { FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { useErrorContext } from "./useErrorContext";
import { Button, Input } from "./fluent";
import { queryKeys, useSecrets } from "./queries";

export default function SecretsView() {
  const { data: secrets = [] } = useSecrets();
  const [showForm, setShowForm] = useState(false);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [pendingValues, setPendingValues] = useState<Record<string, string>>({});
  const queryClient = useQueryClient();
  const { reportError } = useErrorContext();

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: queryKeys.secrets });

  const createMutation = useMutation({
    mutationFn: api.createSecret,
    onSuccess: () => {
      setName("");
      setDescription("");
      setShowForm(false);
      void invalidate();
    },
    onError: reportError,
  });

  const setMutation = useMutation({
    mutationFn: ({ secretName, value }: { secretName: string; value: string }) =>
      api.setSecretValue(secretName, value),
    onSuccess: (_value, variables) => {
      setPendingValues((current) => ({ ...current, [variables.secretName]: "" }));
      void invalidate();
    },
    onError: reportError,
  });

  const clearMutation = useMutation({
    mutationFn: api.clearSecretValue,
    onSuccess: () => invalidate(),
    onError: reportError,
  });

  const deleteMutation = useMutation({
    mutationFn: api.deleteSecret,
    onSuccess: () => invalidate(),
    onError: reportError,
  });

  const busy = createMutation.isPending
    || setMutation.isPending
    || clearMutation.isPending
    || deleteMutation.isPending;

  function createSecret(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    createMutation.mutate({
      name,
      description: description.trim() || null,
    });
  }

  function exportSecret(secretName: string) {
    void api.downloadConfigResource("secret", secretName).catch(reportError);
  }

  return (
    <div className="view-content management-view">
      <div className="view-header">
        <div>
          <h2>Secrets</h2>
          <span className="muted">
            {secrets.length} declarations · {secrets.filter((secret) => secret.is_set).length} values set
          </span>
        </div>
        <Button onClick={() => setShowForm((current) => !current)} type="button">
          {showForm ? "Cancel" : "New Secret"}
        </Button>
      </div>
      <p className="muted management-intro">
        Values are write-only and installation-local. They are never included in YAML exports.
      </p>

      {showForm && (
        <form className="create-form card" onSubmit={createSecret}>
          <label>
            Name
            <Input
              pattern="[A-Z][A-Z0-9_]*"
              placeholder="OPENAI_API_KEY"
              required
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
          <label>
            Description
            <Input
              placeholder="Used by the primary model connection"
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </label>
          <Button disabled={busy} type="submit">Create Declaration</Button>
        </form>
      )}

      <div className="card-grid management-card-grid">
        {secrets.map((secret) => {
          const pendingValue = pendingValues[secret.name] ?? "";
          return (
            <div className="card management-card" key={secret.name}>
              <div className="card-body">
                <div className="management-card-heading">
                  <div className="management-title-block">
                    <h3>{secret.name}</h3>
                    {secret.description && <span className="muted">{secret.description}</span>}
                  </div>
                  <span className={`status-badge ${secret.is_set ? "active" : "error"}`}>
                    {secret.is_set ? "set" : "value required"}
                  </span>
                </div>
                <label>
                  {secret.is_set ? "Replace value" : "Set value"}
                  <Input
                    autoComplete="new-password"
                    placeholder="Value is never displayed"
                    type="password"
                    value={pendingValue}
                    onChange={(event) => setPendingValues((current) => ({
                      ...current,
                      [secret.name]: event.target.value,
                    }))}
                  />
                </label>
                {secret.references.length > 0 && (
                  <div className="card-meta">
                    <strong>Referenced by:</strong>
                    {secret.references.map((reference) => (
                      <code key={reference}>{reference}</code>
                    ))}
                  </div>
                )}
              </div>
              <div className="card-footer">
                <Button
                  className="secondary-button small"
                  disabled={busy || pendingValue.length === 0}
                  onClick={() => setMutation.mutate({
                    secretName: secret.name,
                    value: pendingValue,
                  })}
                  type="button"
                >
                  {secret.is_set ? "Replace" : "Set Value"}
                </Button>
                <div className="card-footer-actions">
                  <Button
                    className="secondary-button small"
                    onClick={() => exportSecret(secret.name)}
                    type="button"
                  >
                    Export YAML
                  </Button>
                  {secret.is_set ? (
                    <Button
                      className="danger-button small"
                      disabled={busy}
                      onClick={() => {
                        if (window.confirm(`Clear ${secret.name}?`)) {
                          clearMutation.mutate(secret.name);
                        }
                      }}
                      type="button"
                    >
                      Clear Value
                    </Button>
                  ) : (
                    <Button
                      className="danger-button small"
                      disabled={busy || secret.references.length > 0}
                      onClick={() => deleteMutation.mutate(secret.name)}
                      type="button"
                    >
                      Delete
                    </Button>
                  )}
                </div>
              </div>
            </div>
          );
        })}
        {secrets.length === 0 && (
          <div className="empty-state">
            No secret declarations. Create one before using a secretRef in configuration.
          </div>
        )}
      </div>
    </div>
  );
}
