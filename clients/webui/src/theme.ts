/*
 * Themes for the console.
 *
 * The console is a tool, not a showcase: every state change should land on the
 * next frame. Fluent drives its CSS transitions from the `duration*` theme
 * tokens, so collapsing them here makes hover, focus and selection feedback
 * instant across every Fluent component without patching individual styles.
 * The JavaScript-driven enter/exit motion used by Dialog, Menu and Popover is
 * switched off separately with `MotionBehaviourProvider value="skip"`.
 */
import { webDarkTheme, webLightTheme } from "@fluentui/react-components";
import type { Theme } from "@fluentui/react-components";

/** Not zero: Fluent waits on `transitionend` in a few places. */
const instant = "1ms";

function withoutMotion(theme: Theme): Theme {
    return {
        ...theme,
        durationUltraFast: instant,
        durationFaster: instant,
        durationFast: instant,
        durationNormal: instant,
        durationGentle: instant,
        durationSlow: instant,
        durationSlower: instant,
        durationUltraSlow: instant,
    };
}

export const lightTheme = withoutMotion(webLightTheme);
export const darkTheme = withoutMotion(webDarkTheme);
