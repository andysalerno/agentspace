import { describe, expect, it } from "vitest";
import {
    formatRelativeAge,
    coverageStateLabel,
    formatCacheSummary,
    formatCompactCount,
    formatContextSummary,
    formatLatestCallSummary,
    formatSessionTotalSummary,
    formatSubagentSummary,
    isSessionTelemetryActive,
    telemetryDisplayReason,
    telemetryDisplayState,
    telemetryStatusAge,
} from "./telemetry";
import type {
    ActivityCounts,
    CacheReportingState,
    CacheSignal,
    CacheSignalConfidence,
    CacheSignalReason,
    CacheSignalState,
    SubagentBreakdown,
    TelemetrySnapshot,
    TelemetryWarning,
    TelemetryWarningCode,
    TelemetryWarningSummary,
    TokenAccountingConvention,
} from "./types";

const CACHE_REPORTING: CacheReportingState = "reported";
const TOKEN_ACCOUNTING: TokenAccountingConvention = "inclusive";
const CACHE_SIGNAL_STATE: CacheSignalState = "healthy";
const CACHE_SIGNAL_CONFIDENCE: CacheSignalConfidence | null = null;
const CACHE_SIGNAL_REASON: CacheSignalReason | null = null;
const WARNING_CODE: TelemetryWarningCode = "malformed_record";
const WARNING_ITEM: TelemetryWarning = {
    code: WARNING_CODE,
    count: 1,
};
const WARNING_SUMMARY: TelemetryWarningSummary = {
    total: 1,
    items: [WARNING_ITEM],
};
const COUNTS: ActivityCounts = {
    interactions: 4,
    model_calls: 7,
    tool_calls: 11,
    subagent_invocations: 1,
    subagent_model_calls: 2,
    errors: 0,
};
const SUBAGENTS: SubagentBreakdown = {
    invocations: 1,
    model_calls: 2,
    effective_input_tokens: 9_200,
    output_tokens: 44,
    cache_read_input_tokens: 9_100,
    cache_write_input_tokens: 50,
    duration_ms: 18_000,
};
const CACHE_SIGNAL: CacheSignal = {
    state: CACHE_SIGNAL_STATE,
    confidence: CACHE_SIGNAL_CONFIDENCE,
    reason: CACHE_SIGNAL_REASON,
};

const SNAPSHOT = {
    schema_version: 1,
    state: "live",
    reason: null,
    content_mode: "metadata",
    source_version: "1.0.81-0",
    observed_at: "2026-08-16T08:00:00Z",
    received_at: "2026-08-16T08:00:02Z",
    session: {
        raw_input_tokens: 48_960,
        effective_input_tokens: 48_216,
        output_tokens: 154,
        total_tokens: 48_370,
        reasoning_output_tokens: 21,
        cache_read_input_tokens: 48_012,
        cache_write_input_tokens: 112,
        other_input_tokens: 92,
        fresh_input_tokens: 204,
        cache_reuse_percent: 99.6,
        nano_aiu: 6_400_000,
        opaque_cost: 0.1472,
    },
    latest_call: {
        started_at: "2026-08-16T07:59:56Z",
        ended_at: "2026-08-16T08:00:00Z",
        duration_ms: 4_000,
        model: "gpt-5.6-sol",
        requested_model: "gpt-5.6-sol",
        provider: "openai",
        agent_id: "builtin:task",
        agent_name: "task",
        is_subagent: false,
        cache_reporting: CACHE_REPORTING,
        token_accounting_convention: TOKEN_ACCOUNTING,
        usage: {
            raw_input_tokens: 16_520,
            effective_input_tokens: 16_512,
            output_tokens: 13,
            total_tokens: 16_525,
            reasoning_output_tokens: 3,
            cache_read_input_tokens: 16_404,
            cache_write_input_tokens: 18,
            other_input_tokens: 90,
            fresh_input_tokens: 108,
            cache_reuse_percent: 99.3,
            nano_aiu: 2_300_000,
            opaque_cost: 0.0412,
        },
    },
    last_interaction: null,
    context: {
        tokens: 17_832,
        limit: 272_000,
        message_count: 18,
        observed_at: "2026-08-16T08:00:00Z",
    },
    counts: COUNTS,
    subagents: SUBAGENTS,
    cache_signal: CACHE_SIGNAL,
    reporting: {
        model_calls: 7,
        cache_reported_calls: 7,
        convention_resolved_calls: 7,
        effective_input_covered_calls: 7,
        context_reported: true,
    },
    warnings: WARNING_SUMMARY,
} satisfies TelemetrySnapshot;

describe("telemetry helpers", () => {
    it("formats compact summaries from reported telemetry", () => {
        expect(formatCompactCount(48_216)).toBe("48.2k");
        expect(formatSessionTotalSummary(SNAPSHOT.session, SNAPSHOT.state)).toBe(
            "48.4k tokens",
        );
        expect(formatLatestCallSummary(SNAPSHOT.latest_call, SNAPSHOT.state)).toBe(
            "16.5k input / 13 output",
        );
        expect(formatCacheSummary(SNAPSHOT, SNAPSHOT.state)).toBe(
            "99.6% (48k read)",
        );
        expect(formatContextSummary(SNAPSHOT.context, SNAPSHOT.state)).toBe(
            "17.8k / 272k",
        );
        expect(formatSubagentSummary(SNAPSHOT, SNAPSHOT.state)).toBe("1 subagent");
    });

    it("preserves null and unavailable states without zero defaults", () => {
        const partial = {
            ...SNAPSHOT,
            session: {
                ...SNAPSHOT.session,
                total_tokens: null,
                cache_reuse_percent: null,
                cache_read_input_tokens: 16_404,
            },
            latest_call: null,
            context: null,
            state: "starting" as const,
            reporting: {
                ...SNAPSHOT.reporting,
                cache_reported_calls: 3,
                convention_resolved_calls: 3,
                effective_input_covered_calls: 3,
            },
        } satisfies TelemetrySnapshot;

        expect(formatSessionTotalSummary(partial.session, partial.state)).toBe(
            "N/A",
        );
        expect(formatLatestCallSummary(partial.latest_call, partial.state)).toBe(
            "N/A",
        );
        expect(formatCacheSummary(partial, "live")).toBe("16.4k read (partial)");
        expect(formatContextSummary(partial.context, partial.state)).toBe(
            "N/A",
        );

        const unavailable = {
            ...partial,
            state: "unavailable" as const,
        } satisfies TelemetrySnapshot;
        expect(formatSessionTotalSummary(unavailable.session, unavailable.state)).toBe(
            "N/A",
        );
        expect(formatLatestCallSummary(unavailable.latest_call, unavailable.state)).toBe(
            "N/A",
        );
    });

    it("derives stale, starting, and unavailable display states independently", () => {
        expect(telemetryDisplayState(SNAPSHOT, new Error("503 upstream"), false)).toBe(
            "stale",
        );
        expect(
            telemetryDisplayReason(SNAPSHOT, new Error("503 upstream"), false),
        ).toContain("Retained browser snapshot");
        expect(telemetryDisplayState(undefined, new Error("503 upstream"), false)).toBe(
            "unavailable",
        );
        expect(telemetryDisplayState(undefined, null, true)).toBe("starting");
    });

    it("labels coverage, age, and activity clearly", () => {
        expect(coverageStateLabel(SNAPSHOT.reporting)).toBe("Complete");
        expect(
            telemetryStatusAge(
                SNAPSHOT,
                0,
                Date.parse("2026-08-16T08:00:05Z"),
            ),
        ).toBe("5s");
        expect(
            formatRelativeAge(
                "2026-08-16T08:00:00Z",
                Date.parse("2026-08-16T08:01:00Z"),
            ),
        ).toBe("1m");
        expect(
            isSessionTelemetryActive("idle", false, { state: "running" }),
        ).toBe(true);
        expect(
            isSessionTelemetryActive("idle", false, { state: "exited" }),
        ).toBe(false);
    });
});
