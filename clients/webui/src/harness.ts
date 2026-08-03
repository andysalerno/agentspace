const ACRONYMS = new Set(["acp", "cli"]);

/** Render a kebab-case harness id as a display label, e.g. `copilot-cli` -> `Copilot CLI`. */
export function formatHarnessLabel(harness: string): string {
    return harness
        .split("-")
        .map((part) =>
            ACRONYMS.has(part)
                ? part.toUpperCase()
                : part.charAt(0).toUpperCase() + part.slice(1)
        )
        .join(" ");
}
