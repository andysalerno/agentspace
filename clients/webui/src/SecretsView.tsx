import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
    Add20Regular,
    ArrowDownload20Regular,
    Delete20Regular,
    EraserRegular,
    Key20Regular,
    Password24Regular,
} from "@fluentui/react-icons";
import { api } from "./api";
import { useErrorContext } from "./useErrorContext";
import { Button, Field, Input } from "./fluent";
import { queryKeys, useSecrets } from "./queries";
import { EmptyState, FormDialog, RowActions, StatusBadge, ViewHeader } from "./ui";

export default function SecretsView() {
    const { data: secrets = [] } = useSecrets();
    const queryClient = useQueryClient();
    const { reportError } = useErrorContext();

    const [showForm, setShowForm] = useState(false);
    const [name, setName] = useState("");
    const [description, setDescription] = useState("");
    const [valueTarget, setValueTarget] = useState<string | null>(null);
    const [pendingValue, setPendingValue] = useState("");

    const invalidate = () => queryClient.invalidateQueries({ queryKey: queryKeys.secrets });

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
        onSuccess: () => {
            setValueTarget(null);
            setPendingValue("");
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

    const setCount = secrets.filter((secret) => secret.is_set).length;

    return (
        <div className="view-content">
            <ViewHeader
                actions={
                    <Button
                        appearance="primary"
                        icon={<Add20Regular />}
                        onClick={() => setShowForm(true)}
                        type="button"
                    >
                        New secret
                    </Button>
                }
                description={`${secrets.length} declarations, ${setCount} with a value. Values are write-only, installation-local, and never included in YAML exports.`}
                title="Secrets"
            />
            <div className="view-body">
                {secrets.length === 0
                    ? (
                        <EmptyState
                            action={
                                <Button appearance="primary" onClick={() => setShowForm(true)}>
                                    New secret
                                </Button>
                            }
                            description="Declare a secret before referencing it from a connection or gateway."
                            icon={<Password24Regular />}
                            title="No secrets declared"
                        />
                    )
                    : (
                        <div className="table-container">
                            <div className="table-scroll">
                                <table className="data-table">
                                    <thead>
                                        <tr>
                                            <th>Secret</th>
                                            <th>Value</th>
                                            <th>Referenced by</th>
                                            <th aria-label="Actions" />
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {secrets.map((secret) => (
                                            <tr key={secret.name}>
                                                <td>
                                                    <div className="cell-identity">
                                                        <span className="cell-identity-name mono-sm">
                                                            {secret.name}
                                                        </span>
                                                        {secret.description !== null
                                                            && secret.description !== ""
                                                            && (
                                                                <span className="cell-identity-id">
                                                                    {secret.description}
                                                                </span>
                                                            )}
                                                    </div>
                                                </td>
                                                <td>
                                                    <StatusBadge
                                                        label={secret.is_set ? "Set" : "Not set"}
                                                        tone={secret.is_set ? "ok" : "warn"}
                                                    />
                                                </td>
                                                <td className="cell-wrap">
                                                    {secret.references.length === 0
                                                        ? <span className="muted">—</span>
                                                        : (
                                                            <span className="tag-row">
                                                                {secret.references.map((ref) => (
                                                                    <span
                                                                        className="tag"
                                                                        key={ref}
                                                                    >
                                                                        {ref}
                                                                    </span>
                                                                ))}
                                                            </span>
                                                        )}
                                                </td>
                                                <td className="actions-cell">
                                                    <RowActions
                                                        items={[
                                                            {
                                                                key: "export",
                                                                label: "Export YAML",
                                                                icon: <ArrowDownload20Regular />,
                                                                onClick: () => {
                                                                    void api.downloadConfigResource(
                                                                        "secret",
                                                                        secret.name,
                                                                    ).catch(reportError);
                                                                },
                                                            },
                                                            ...(secret.is_set
                                                                ? [{
                                                                    key: "clear",
                                                                    label: "Clear value",
                                                                    icon: <EraserRegular />,
                                                                    destructive: true,
                                                                    disabled: busy,
                                                                    onClick: () => {
                                                                        clearMutation.mutate(
                                                                            secret.name,
                                                                        );
                                                                    },
                                                                }]
                                                                : []),
                                                            {
                                                                key: "delete",
                                                                label: secret.references.length > 0
                                                                    ? "Delete (still referenced)"
                                                                    : "Delete declaration",
                                                                icon: <Delete20Regular />,
                                                                destructive: true,
                                                                disabled: busy
                                                                    || secret.references.length > 0,
                                                                onClick: () =>
                                                                    deleteMutation.mutate(
                                                                        secret.name,
                                                                    ),
                                                            },
                                                        ]}
                                                        primary={{
                                                            key: "set",
                                                            label: secret.is_set
                                                                ? "Replace value"
                                                                : "Set value",
                                                            icon: <Key20Regular />,
                                                            disabled: busy,
                                                            onClick: () => {
                                                                setPendingValue("");
                                                                setValueTarget(secret.name);
                                                            },
                                                        }}
                                                    />
                                                </td>
                                            </tr>
                                        ))}
                                    </tbody>
                                </table>
                            </div>
                        </div>
                    )}
            </div>

            <FormDialog
                busy={busy}
                onOpenChange={setShowForm}
                onSubmit={() =>
                    createMutation.mutate({
                        name,
                        description: description.trim() || null,
                    })}
                open={showForm}
                submitLabel="Create declaration"
                title="New secret"
            >
                <Field
                    hint="Uppercase letters, digits, and underscores."
                    label="Name"
                    required
                >
                    <Input
                        onChange={(event) => setName(event.target.value)}
                        pattern="[A-Z][A-Z0-9_]*"
                        placeholder="OPENAI_API_KEY"
                        required
                        value={name}
                    />
                </Field>
                <Field label="Description">
                    <Input
                        onChange={(event) => setDescription(event.target.value)}
                        placeholder="Used by the primary model connection"
                        value={description}
                    />
                </Field>
            </FormDialog>

            <FormDialog
                busy={busy || pendingValue.length === 0}
                onOpenChange={(open) => {
                    if (!open) setValueTarget(null);
                }}
                onSubmit={() => {
                    if (valueTarget === null) return;
                    setMutation.mutate({ secretName: valueTarget, value: pendingValue });
                }}
                open={valueTarget !== null}
                submitLabel="Save value"
                title={`Set value — ${valueTarget ?? ""}`}
            >
                <Field
                    hint="Stored on the server and never returned to the browser."
                    label="Value"
                    required
                >
                    <Input
                        autoComplete="new-password"
                        onChange={(event) => setPendingValue(event.target.value)}
                        placeholder="Value is never displayed"
                        required
                        type="password"
                        value={pendingValue}
                    />
                </Field>
            </FormDialog>
        </div>
    );
}
