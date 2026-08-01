import type { SecretStatus } from "./types";
import { Select } from "./fluent";
import { useSecrets } from "./queries";

type SecretRefSelectProps = {
    /// Name of the referenced secret, or "" when nothing is referenced.
    value: string;
    onChange: (secretName: string) => void;
    /// Label for the "nothing referenced" option.
    noneLabel?: string;
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
    value,
    onChange,
    noneLabel = "None",
    required = false,
    disabled = false,
}: SecretRefSelectProps) {
    const { data: secrets = [] } = useSecrets();
    const declared = secrets.some((secret) => secret.name === value);

    return (
        <>
            <Select
                disabled={disabled}
                required={required}
                value={value}
                onChange={(event) => onChange(event.target.value)}
            >
                <option value="">{noneLabel}</option>
                {secrets.map((secret) => (
                    <option key={secret.name} value={secret.name}>
                        {optionLabel(secret)}
                    </option>
                ))}
                {value !== "" && !declared && (
                    <option value={value}>{value} (not declared)</option>
                )}
            </Select>
            <span className="muted">
                {secrets.length === 0
                    ? "No secrets declared yet. Declare one on the Secrets page first."
                    : "References a declared secret by name. Values are set on the Secrets page and are never sent to the browser."}
            </span>
        </>
    );
}
