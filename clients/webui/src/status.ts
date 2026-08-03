/** Maps backend status strings onto the console's small tone vocabulary. */
import type { StatusTone } from "./ui";

const OK = new Set(["active", "running", "ready", "connected", "healthy"]);
const WARN = new Set(["starting", "creating", "busy", "pending", "restarting", "stopping"]);
const BAD = new Set(["error", "failed", "dead", "invalid", "unreachable"]);

export function statusTone(status: string): StatusTone {
    const value = status.toLowerCase();
    if (OK.has(value)) return "ok";
    if (WARN.has(value)) return "warn";
    if (BAD.has(value)) return "error";
    return "neutral";
}

/** Idle sessions are healthy but not doing work, so they read as neutral. */
export function sessionTone(status: string): StatusTone {
    if (status === "idle") return "neutral";
    return statusTone(status);
}
