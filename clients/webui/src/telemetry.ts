import type {
    ContextUsage,
    ModelCallSummary,
    ReportingCoverage,
    TelemetryContentMode,
    TelemetrySnapshot,
    TelemetryState,
    TerminalStatus,
    UsageBreakdown,
} from "./types";

export const ACTIVE_TELEMETRY_POLL_MS = 2_000;
export const IDLE_TELEMETRY_POLL_MS = 5_000;

export const CACHE_REUSE_TOOLTIP = "Share of effective input tokens served from cache. This is not the percentage of requests with a cache hit.";
export const CONTEXT_OCCUPANCY_TOOLTIP = "Reported by Copilot as of the last model call.";

const ACTIVE_SESSION_STATUSES = new Set(["active", "busy", "running", "working"]);
const integerFormatter = new Intl.NumberFormat("en-US");
const costFormatter = new Intl.NumberFormat("en-US", {
    minimumFractionDigits: 0,
    maximumFractionDigits: 4,
});
const percentFormatter = new Intl.NumberFormat("en-US", {
    minimumFractionDigits: 0,
    maximumFractionDigits: 1,
});

function errorMessage(error: unknown): string {
    if (error instanceof Error) {
        return error.message;
    }
    if (typeof error === "string") {
        return error;
    }
    return "Unknown telemetry error";
}

function withSuffix(value: number, divisor: number, suffix: string): string {
    const scaled = value / divisor;
    const decimals = scaled >= 100 ? 0 : (scaled >= 10 ? 1 : 1);
    return `${scaled.toFixed(decimals).replace(/\.0$/, "")}${suffix}`;
}

export function formatCompactCount(value: number): string {
    const absolute = Math.abs(value);
    if (absolute >= 1_000_000_000) {
        return withSuffix(value, 1_000_000_000, "b");
    }
    if (absolute >= 1_000_000) {
        return withSuffix(value, 1_000_000, "m");
    }
    if (absolute >= 1_000) {
        return withSuffix(value, 1_000, "k");
    }
    return integerFormatter.format(value);
}

export function formatExactCount(value: number | null): string {
    return value === null ? "N/A" : integerFormatter.format(value);
}

export function formatPercent(value: number | null): string {
    return value === null ? "N/A" : `${percentFormatter.format(value)}%`;
}

export function formatOpaqueCost(value: number | null): string {
    return value === null ? "N/A" : costFormatter.format(value);
}

export function formatDuration(durationMs: number | null): string {
    if (durationMs === null) {
        return "N/A";
    }
    if (durationMs < 1_000) {
        return `${integerFormatter.format(durationMs)}ms`;
    }
    if (durationMs < 60_000) {
        return `${(durationMs / 1_000).toFixed(durationMs >= 10_000 ? 0 : 1).replace(/\.0$/, "")}s`;
    }
    const minutes = Math.floor(durationMs / 60_000);
    const seconds = Math.floor((durationMs % 60_000) / 1_000);
    if (minutes < 60) {
        return seconds === 0 ? `${minutes}m` : `${minutes}m ${seconds}s`;
    }
    const hours = Math.floor(minutes / 60);
    const remainingMinutes = minutes % 60;
    return remainingMinutes === 0 ? `${hours}h` : `${hours}h ${remainingMinutes}m`;
}

export function formatRelativeAge(timestamp: string | null, nowMs = Date.now()): string | null {
    if (timestamp === null) {
        return null;
    }
    const parsed = Date.parse(timestamp);
    if (Number.isNaN(parsed)) {
        return null;
    }
    const elapsedMs = Math.max(0, nowMs - parsed);
    const seconds = Math.floor(elapsedMs / 1_000);
    if (seconds < 60) {
        return `${seconds}s`;
    }
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) {
        return `${minutes}m`;
    }
    const hours = Math.floor(minutes / 60);
    if (hours < 24) {
        return `${hours}h`;
    }
    const days = Math.floor(hours / 24);
    return `${days}d`;
}

export function humanizeSnakeCase(value: string): string {
    return value
        .split("_")
        .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
        .join(" ");
}

export function telemetryContentModeLabel(value: TelemetryContentMode): string {
    return humanizeSnakeCase(value);
}

export function telemetryStatusAge(
    snapshot: TelemetrySnapshot | undefined,
    dataUpdatedAt: number,
    nowMs = Date.now(),
): string | null {
    const timestamp = snapshot?.observed_at
        ?? snapshot?.received_at
        ?? (dataUpdatedAt > 0 ? new Date(dataUpdatedAt).toISOString() : null);
    return formatRelativeAge(timestamp, nowMs);
}

export function telemetryDisplayState(
    snapshot: TelemetrySnapshot | undefined,
    error: unknown,
    isPending: boolean,
): TelemetryState {
    if (snapshot !== undefined && error) {
        return "stale";
    }
    if (snapshot !== undefined) {
        return snapshot.state;
    }
    if (error) {
        return "unavailable";
    }
    if (isPending) {
        return "starting";
    }
    return "unavailable";
}

export function telemetryDisplayReason(
    snapshot: TelemetrySnapshot | undefined,
    error: unknown,
    _isPending: boolean,
): string | null {
    if (snapshot !== undefined && error instanceof Error) {
        return `Retained browser snapshot while telemetry retries after: ${error.message}`;
    }
    if (snapshot !== undefined && error !== null && error !== undefined) {
        return `Retained browser snapshot while telemetry retries after: ${errorMessage(error)}`;
    }
    if (snapshot?.reason !== null && snapshot?.reason !== undefined) {
        return snapshot.reason;
    }
    if (error instanceof Error) {
        return error.message;
    }
    if (error !== null && error !== undefined) {
        return errorMessage(error);
    }
    return null;
}

export function isSessionTelemetryActive(
    sessionStatus: string | null | undefined,
    hasActiveTurn: boolean,
    terminalStatus: Pick<TerminalStatus, "state"> | null,
): boolean {
    if (hasActiveTurn) {
        return true;
    }
    if (terminalStatus?.state === "running") {
        return true;
    }
    return sessionStatus !== null
        && sessionStatus !== undefined
        && ACTIVE_SESSION_STATUSES.has(sessionStatus.toLowerCase());
}

export function formatSessionTotalSummary(
    usage: UsageBreakdown,
    _state: TelemetryState,
): string {
    if (usage.total_tokens !== null) {
        return `${formatCompactCount(usage.total_tokens)} tokens`;
    }
    return "N/A";
}

export function formatLatestCallSummary(
    latestCall: ModelCallSummary | null,
    _state: TelemetryState,
): string {
    if (latestCall === null) {
        return "N/A";
    }
    const input = latestCall.usage.effective_input_tokens;
    const output = latestCall.usage.output_tokens;
    if (input !== null && output !== null) {
        return `${formatCompactCount(input)} input / ${formatCompactCount(output)} output`;
    }
    if (input !== null) {
        return `${formatCompactCount(input)} input / N/A`;
    }
    if (output !== null) {
        return `N/A / ${formatCompactCount(output)} output`;
    }
    return "N/A";
}

export function formatCacheSummary(
    snapshot: TelemetrySnapshot | undefined,
    _state: TelemetryState,
): string {
    if (snapshot === undefined) {
        return "N/A";
    }
    if (snapshot.state === "unavailable") {
        return "N/A";
    }
    const percent = snapshot.session.cache_reuse_percent;
    const read = snapshot.session.cache_read_input_tokens;
    if (percent !== null && read !== null) {
        return `${formatPercent(percent)} (${formatCompactCount(read)} read)`;
    }
    if (percent !== null) {
        return `${formatPercent(percent)} cache`;
    }
    if (read !== null && snapshot.reporting.model_calls > 0) {
        return `${formatCompactCount(read)} read (partial)`;
    }
    if (snapshot.reporting.model_calls === 0) {
        return "N/A";
    }
    return "N/A";
}

export function formatContextSummary(
    context: ContextUsage | null,
    _state: TelemetryState,
): string {
    if (context === null) {
        return "N/A";
    }
    if (context.tokens !== null && context.limit !== null) {
        return `${formatCompactCount(context.tokens)} / ${formatCompactCount(context.limit)}`;
    }
    if (context.tokens !== null) {
        return `${formatCompactCount(context.tokens)} / N/A`;
    }
    if (context.limit !== null) {
        return `N/A / ${formatCompactCount(context.limit)}`;
    }
    return "N/A";
}

export function formatSubagentSummary(
    snapshot: TelemetrySnapshot | undefined,
    _state: TelemetryState,
): string {
    if (snapshot === undefined || snapshot.state === "unavailable") {
        return "N/A";
    }
    const count = snapshot.counts.subagent_invocations;
    return count === 1 ? "1 subagent" : `${integerFormatter.format(count)} subagents`;
}

export function formatCoverage(covered: number, total: number): string {
    if (total === 0) {
        return "N/A";
    }
    if (covered === total) {
        return `All ${integerFormatter.format(total)}`;
    }
    return `${integerFormatter.format(covered)} of ${integerFormatter.format(total)}`;
}

export function formatUsageValue(
    value: number | null,
    kind: "count" | "percent" | "cost" = "count",
): string {
    switch (kind) {
        case "percent":
            return formatPercent(value);
        case "cost":
            return formatOpaqueCost(value);
        case "count":
        default:
            return formatExactCount(value);
    }
}

export function usageReported(usage: UsageBreakdown): boolean {
    return Object.values(usage).some((value) => value !== null);
}

export function shouldShowObservedCounts(snapshot: TelemetrySnapshot): boolean {
    return snapshot.state !== "unavailable";
}

export function formatWarnings(snapshot: TelemetrySnapshot): string {
    if (snapshot.warnings.total === 0 || snapshot.warnings.items.length === 0) {
        return "None";
    }
    return snapshot.warnings.items
        .map((warning) => `${humanizeSnakeCase(warning.code)} ×${integerFormatter.format(warning.count)}`)
        .join("; ");
}

export function coverageStateLabel(
    reporting: ReportingCoverage,
): string {
    if (reporting.model_calls === 0) {
        return "N/A";
    }
    if (
        reporting.cache_reported_calls === reporting.model_calls
        && reporting.convention_resolved_calls === reporting.model_calls
        && reporting.effective_input_covered_calls === reporting.model_calls
    ) {
        return "Complete";
    }
    return "Partial";
}
