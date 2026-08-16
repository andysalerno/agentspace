import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "./api";
import type { TelemetrySnapshot } from "./types";

const SNAPSHOT = {
    schema_version: 1,
    state: "live",
    reason: null,
    content_mode: "metadata",
    source_version: "1.0.81-0",
    observed_at: "2026-08-16T08:00:00Z",
    received_at: "2026-08-16T08:00:01Z",
    session: {
        raw_input_tokens: 12,
        effective_input_tokens: 9,
        output_tokens: 3,
        total_tokens: 12,
        reasoning_output_tokens: 1,
        cache_read_input_tokens: 2,
        cache_write_input_tokens: 1,
        other_input_tokens: 6,
        fresh_input_tokens: 7,
        cache_reuse_percent: 22.2,
        nano_aiu: 4,
        opaque_cost: 0.5,
    },
    latest_call: null,
    last_interaction: null,
    context: null,
    counts: {
        interactions: 0,
        model_calls: 0,
        tool_calls: 0,
        subagent_invocations: 0,
        subagent_model_calls: 0,
        errors: 0,
    },
    subagents: {
        invocations: 0,
        model_calls: 0,
        effective_input_tokens: null,
        output_tokens: null,
        cache_read_input_tokens: null,
        cache_write_input_tokens: null,
        duration_ms: null,
    },
    cache_signal: null,
    reporting: {
        model_calls: 0,
        cache_reported_calls: 0,
        convention_resolved_calls: 0,
        effective_input_covered_calls: 0,
        context_reported: false,
    },
    warnings: {
        total: 0,
        items: [],
    },
} satisfies TelemetrySnapshot;

afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

describe("api.getSessionTelemetry", () => {
    it("requests the normalized telemetry endpoint", async () => {
        const fetchMock = vi.fn().mockResolvedValue({
            ok: true,
            status: 200,
            json: () => Promise.resolve(SNAPSHOT),
        });
        vi.stubGlobal("fetch", fetchMock);

        await expect(api.getSessionTelemetry("cli-session")).resolves.toEqual(SNAPSHOT);

        expect(fetchMock).toHaveBeenCalledWith("/api/sessions/cli-session/telemetry", {
            headers: {
                "Content-Type": "application/json",
            },
        });
    });

    it("surfaces service errors without mutating terminal state", async () => {
        const fetchMock = vi.fn().mockResolvedValue({
            ok: false,
            status: 503,
            statusText: "Service Unavailable",
            text: () => Promise.resolve(JSON.stringify({ detail: "agent_host returned HTTP 503" })),
        });
        vi.stubGlobal("fetch", fetchMock);

        await expect(api.getSessionTelemetry("cli-session")).rejects.toEqual(
            expect.objectContaining({
                name: "ApiError",
                status: 503,
                message: "agent_host returned HTTP 503",
            }),
        );
    });
});
