import type { SecretStatus } from "./types";
import { Field, Select } from "./fluent";
import { useSecrets } from "./queries";

/// Sentinel for a configured value the picker cannot express, namely a literal
/// authored in YAML. It cannot collide with a secret name, which must match
/// `[A-Z][A-Z0-9_]*`.
export const LITERAL_VALUE = "__literal__";

type SecretRefSelectProps = {
    /// Field label. Rendered by the shared Fluent `Field` wrapper.
    label: string;
    /// Name of the referenced secret, "" when nothing is referenced, or
    /// [`LITERAL_VALUE`] when a literal is configured in YAML.
    value: string;
    onChange: (secretName: string) => void;
    /// Label for the "nothing referenced" option.
    noneLabel?: string;
    /// Label for the literal option. Required to select [`LITERAL_VALUE`], which
    /// callers use to keep an existing YAML literal untouched.
    literalLabel?: string;
    required?: boolean;
    disabled?: boolean;
};

function optionLabel(secret: SecretStatus) {
    return secret.is_set ? secret.name : `${secret.name} (value not set)`;
}

/// Picker for a declared secret, used everywhere a configuration field accepts a
/// `secretRef`. Clients deliberately cannot enter literal secret values: a
/// literal would be persisted and exported in plain text, so it stays a
/// YAML-only, explicitly authored choice.
export default function SecretRefSelect({
    label,
    value,
    onChange,
    noneLabel = "None",
    literalLabel,
    required = false,
    disabled = false,
}: SecretRefSelectProps) {
    const { data: secrets = [] } = useSecrets();
    const isLiteral = value === LITERAL_VALUE;
    const declared = secrets.some((secret) => secret.name === value);
    const hint = isLiteral
        ? "Authored as a literal in YAML. Pick a secret to replace it, or clear it here."
        : (secrets.length === 0
            ? "No secrets declared yet. Declare one on the Secrets page first."
            : "Values are set on the Secrets page and are never sent to the browser.");

    return (
        <Field hint={hint} label={label} required={required}>
            <Select
                disabled={disabled}
                onChange={(event) => onChange(event.target.value)}
                required={required}
                value={value}
            >
                <option value="">{noneLabel}</option>
                {literalLabel !== undefined && (
                    <option value={LITERAL_VALUE}>{literalLabel}</option>
                )}
                {secrets.map((secret) => (
                    <option key={secret.name} value={secret.name}>
                        {optionLabel(secret)}
                    </option>
                ))}
                {value !== "" && !isLiteral && !declared && (
                    <option value={value}>{value} (not declared)</option>
                )}
            </Select>
        </Field>
    );
}
