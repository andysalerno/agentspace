import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider, focusManager, onlineManager } from "@tanstack/react-query";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "./api";
import { useSessionTelemetry } from "./queries";
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

function wrapper() {
    const client = new QueryClient({
        defaultOptions: {
            queries: {
                retry: false,
            },
        },
    });

    function TestWrapper({ children }: { children: ReactNode }) {
        return (
            <QueryClientProvider client={client}>{children}</QueryClientProvider>
        );
    }

    return TestWrapper;
}

function HookHarness(
    { sessionId, active }: { sessionId: string | null; active: boolean },
) {
    const query = useSessionTelemetry(sessionId, { active });
    return <div>{query.data?.state ?? "pending"}</div>;
}

let visibilityState = "visible";

function setVisibility(next: "visible" | "hidden") {
    visibilityState = next;
    document.dispatchEvent(new Event("visibilitychange"));
}

async function flushQueryWork() {
    await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
    });
}

beforeEach(() => {
    visibilityState = "visible";
    vi.useFakeTimers();
    Object.defineProperty(document, "visibilityState", {
        configurable: true,
        get: () => visibilityState,
    });
    vi.spyOn(api, "getSessionTelemetry").mockResolvedValue(SNAPSHOT);
});

afterEach(() => {
    cleanup();
    focusManager.setFocused(true);
    onlineManager.setOnline(true);
    vi.useRealTimers();
    vi.restoreAllMocks();
});

describe("useSessionTelemetry", () => {
    it("polls active sessions every 2 seconds, idle sessions every 5 seconds, and suspends when hidden or unselected", async () => {
        const rendered = render(
            <HookHarness active sessionId="cli-session" />,
            { wrapper: wrapper() },
        );

        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(1);

        act(() => {
            vi.advanceTimersByTime(2_000);
        });
        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(2);

        rendered.rerender(<HookHarness active={false} sessionId="cli-session" />);
        await flushQueryWork();

        act(() => {
            vi.advanceTimersByTime(4_000);
        });
        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(2);

        act(() => {
            vi.advanceTimersByTime(1_000);
        });
        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(3);

        act(() => {
            setVisibility("hidden");
            vi.advanceTimersByTime(10_000);
        });
        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(3);

        rendered.rerender(<HookHarness active={false} sessionId={null} />);
        await flushQueryWork();

        act(() => {
            setVisibility("visible");
            vi.advanceTimersByTime(10_000);
        });
        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(3);
    });

    it("refetches immediately on focus and reconnect", async () => {
        render(
            <HookHarness active={false} sessionId="cli-session" />,
            { wrapper: wrapper() },
        );

        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(1);

        act(() => {
            vi.advanceTimersByTime(1_000);
            focusManager.setFocused(false);
            focusManager.setFocused(true);
        });
        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(2);

        act(() => {
            vi.advanceTimersByTime(1_000);
            onlineManager.setOnline(false);
            onlineManager.setOnline(true);
        });
        await flushQueryWork();
        expect(api.getSessionTelemetry).toHaveBeenCalledTimes(3);
    });
});
