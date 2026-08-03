import { createContext, useContext } from "react";

/*
 * Monaco is outside the Fluent theme, so it needs the dark/light choice
 * handed to it directly. Reading `data-theme` off the document during render
 * would leave editors a toggle behind: App writes that attribute from an
 * effect, after its descendants have already rendered, and mutating an
 * attribute does not re-render anything.
 */
export const DarkModeContext = createContext(false);

/** Monaco's built-in theme name matching the current Fluent theme. */
export function useMonacoTheme(): "vs-dark" | "light" {
    return useContext(DarkModeContext) ? "vs-dark" : "light";
}
