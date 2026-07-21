// Required env var keys per harness. Pre-filled (with empty values) into
// Environment Variables editors as a presentation-level hint so the user can
// see what they need to set. The kernel itself enforces these at runtime;
// this map only drives UI affordances.
const REQUIRED_ENV_KEYS_BY_HARNESS: Record<string, string[]> = {
    acp: ["KERNEL_ACP_SERVER", "KERNEL_ACP_MODEL_NAME"],
    opencode: ["KERNEL_OPENCODE_MODEL_NAME"],
};

const MODEL_ENV_KEY_BY_HARNESS: Record<string, string> = {
    acp: "KERNEL_ACP_MODEL_NAME",
    opencode: "KERNEL_OPENCODE_MODEL_NAME",
};

export function modelEnvKeyForHarness(harness: string): string | null {
    return MODEL_ENV_KEY_BY_HARNESS[harness] ?? null;
}

export function getEnvValue(envVars: string, key: string): string {
    let value = "";
    for (const rawLine of envVars.split("\n")) {
        const line = rawLine.trim();
        if (line === "" || line.startsWith("#")) {
            continue;
        }
        const eq = line.indexOf("=");
        const envKey = (eq === -1 ? line : line.slice(0, eq)).trim();
        if (envKey === key) {
            value = eq === -1 ? "" : line.slice(eq + 1);
        }
    }
    return value;
}

export function setEnvValue(envVars: string, key: string, value: string): string {
    const lines = envVars.split("\n");
    let found = false;
    const updated = lines.map((rawLine) => {
        const line = rawLine.trim();
        if (line === "" || line.startsWith("#")) {
            return rawLine;
        }
        const eq = line.indexOf("=");
        const envKey = (eq === -1 ? line : line.slice(0, eq)).trim();
        if (envKey !== key) {
            return rawLine;
        }
        found = true;
        return `${key}=${value}`;
    });
    if (found) {
        return updated.join("\n");
    }
    const prefix = envVars === "" || envVars.endsWith("\n") ? envVars : envVars + "\n";
    return `${prefix}${key}=${value}\n`;
}

/**
 * Merge any required keys for the harness into an env-vars text blob, adding
 * `KEY=` lines for keys that are not already present. Existing lines (and
 * their values) are preserved untouched.
 */
export function withRequiredEnvKeys(envVars: string, harness: string): string {
    const required = REQUIRED_ENV_KEYS_BY_HARNESS[harness];
    if (!required || required.length === 0) {
        return envVars;
    }
    const present = new Set<string>();
    for (const rawLine of envVars.split("\n")) {
        const line = rawLine.trim();
        if (line === "" || line.startsWith("#")) {
            continue;
        }
        const eq = line.indexOf("=");
        const key = (eq === -1 ? line : line.slice(0, eq)).trim();
        if (key !== "") present.add(key);
    }
    const additions = required.filter((k) => !present.has(k)).map((k) => `${k}=`);
    if (additions.length === 0) {
        return envVars;
    }
    const prefix = envVars === "" || envVars.endsWith("\n") ? envVars : envVars + "\n";
    return prefix + additions.join("\n") + "\n";
}
