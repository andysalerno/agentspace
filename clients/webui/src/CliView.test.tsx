import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import CliView from "./CliView";
import SessionsView from "./SessionsView";
import { api } from "./api";
import { IN_DIALOG } from "./dialogTestQuery";
import { ErrorProvider } from "./ErrorContext";
import { FluentProvider } from "./fluent";
import { lightTheme } from "./theme";
import type {
    Agent,
    KernelSummary,
    SessionDetail,
    SessionSummary,
    TelemetrySnapshot,
    TerminalAttachKind,
    TerminalStatus,
} from "./types";

const terminalMock = vi.hoisted(() => {
    const cleanup = vi.fn<(sessionId: string) => void>();
    const state: {
        attachKind: TerminalAttachKind | null;
        cleanup: typeof cleanup;
    } = {
        attachKind: "started",
        cleanup,
    };
    return state;
});

vi.mock("./Terminal", async () => {
    const React = await import("react");
    return {
        default: function MockTerminal(props: {
            sessionId: string;
            onAttachmentChange: (attachment: unknown) => void;
            onConnectionStateChange: (state: string) => void;
            onLifecycleStatus: (status: TerminalStatus) => void;
        }) {
            const {
                onAttachmentChange,
                onConnectionStateChange,
                onLifecycleStatus,
                sessionId,
            } = props;
            React.useEffect(() => {
                const status: TerminalStatus = {
                    state: "running",
                    exit_status: null,
                    attach_kind: terminalMock.attachKind,
                    attachment_count: 1,
                };
                onAttachmentChange({
                    attachmentId: "attachment-ready",
                    cols: 100,
                    rows: 30,
                    terminal: status,
                });
                onLifecycleStatus(status);
                onConnectionStateChange("ready");
                return () => {
                    terminalMock.cleanup(sessionId);
                    onAttachmentChange(null);
                };
            }, [
                onAttachmentChange,
                onConnectionStateChange,
                onLifecycleStatus,
                sessionId,
            ]);
            return (
                <div className="terminal-shell">
                    <div className="terminal-canvas">
                        <div aria-label="Mock terminal">connected terminal</div>
                    </div>
                    <div className="terminal-status">Terminal ready</div>
                </div>
            );
        },
    };
});

function wrapper() {
    const client = new QueryClient({
        defaultOptions: {
            queries: { retry: false },
            mutations: { retry: false },
        },
    });
    function TestWrapper({ children }: { children: ReactNode }) {
        return (
            <FluentProvider theme={lightTheme}>
                <QueryClientProvider client={client}>
                    <ErrorProvider>{children}</ErrorProvider>
                </QueryClientProvider>
            </FluentProvider>
        );
    }
    return TestWrapper;
}

const CLI_AGENT: Agent = {
    agent_id: "cli-agent",
    name: "CLI Agent",
    harness: "acp",
    system_prompt: "Use the terminal.",
    skills: [],
    env_vars: "",
    connection_id: null,
    cli: { harness: "copilot-cli", connection_id: null },
    workspace_mounts: [],
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
};

const CHAT_AGENT: Agent = {
    ...CLI_AGENT,
    agent_id: "chat-agent",
    name: "Chat Only Agent",
    cli: null,
};

const CLI_SESSION: SessionSummary = {
    session_id: "session-1234567890abcdefghijkl",
    agent_id: "cli-agent",
    status: "running",
    channel_name: null,
    client_type: "webui",
    interaction_mode: "cli",
    cli_harness: "copilot-cli",
    cli_connection_id: null,
    harness_session_id: "copilot-abcdef1234567890",
    runtime_generation: 3,
    runtime_status: "live",
    recovery_state: "recoverable",
    vscode_url: "http://127.0.0.1:8100",
    free_port_url: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    message_count: 0,
};

const CHAT_SESSION: SessionSummary = {
    ...CLI_SESSION,
    session_id: "chat-session",
    agent_id: "chat-agent",
    interaction_mode: "chat",
    cli_harness: null,
    harness_session_id: null,
    runtime_generation: null,
    runtime_status: null,
};

const KERNEL: KernelSummary = {
    session_id: CLI_SESSION.session_id,
    harness: "copilot-cli",
    status: "running",
    turns: 0,
    resume_token: null,
    additional_paths: ["/workspace"],
    client_session_ids: [CLI_SESSION.session_id],
    channel_names: [],
    agent_ids: ["cli-agent"],
    container_name: "agentspace-cli",
    vscode_url: "http://127.0.0.1:8100",
    free_port_url: null,
    stats: null,
};

const RUNNING_TERMINAL: TerminalStatus = {
    state: "running",
    exit_status: null,
    attach_kind: "started",
    attachment_count: 1,
};

const LIVE_TELEMETRY: TelemetrySnapshot = {
    schema_version: 1,
    state: "live",
    reason: null,
    content_mode: "metadata",
    source_version: "1.0.81-0",
    observed_at: "2026-08-16T08:00:00Z",
    received_at: "2026-08-16T08:00:01Z",
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
        cache_reporting: "reported",
        token_accounting_convention: "inclusive",
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
    counts: {
        interactions: 4,
        model_calls: 7,
        tool_calls: 11,
        subagent_invocations: 1,
        subagent_model_calls: 2,
        errors: 0,
    },
    subagents: {
        invocations: 1,
        model_calls: 2,
        effective_input_tokens: 9_200,
        output_tokens: 44,
        cache_read_input_tokens: 9_100,
        cache_write_input_tokens: 50,
        duration_ms: 18_000,
    },
    cache_signal: {
        state: "healthy",
        confidence: null,
        reason: null,
    },
    reporting: {
        model_calls: 7,
        cache_reported_calls: 7,
        convention_resolved_calls: 7,
        effective_input_covered_calls: 7,
        context_reported: true,
    },
    warnings: {
        total: 0,
        items: [],
    },
};

const UNAVAILABLE_TELEMETRY: TelemetrySnapshot = {
    ...LIVE_TELEMETRY,
    state: "unavailable",
    reason: "telemetry is unavailable for harness echo",
    observed_at: null,
    received_at: null,
    session: {
        raw_input_tokens: null,
        effective_input_tokens: null,
        output_tokens: null,
        total_tokens: null,
        reasoning_output_tokens: null,
        cache_read_input_tokens: null,
        cache_write_input_tokens: null,
        other_input_tokens: null,
        fresh_input_tokens: null,
        cache_reuse_percent: null,
        nano_aiu: null,
        opaque_cost: null,
    },
    latest_call: null,
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
};

function detail(session = CLI_SESSION): SessionDetail {
    return { ...session, messages: [] };
}

beforeEach(() => {
    terminalMock.attachKind = "started";
    terminalMock.cleanup.mockReset();
    vi.spyOn(api, "listAgents").mockResolvedValue([CLI_AGENT, CHAT_AGENT]);
    vi.spyOn(api, "listSessions").mockResolvedValue([CLI_SESSION, CHAT_SESSION]);
    vi.spyOn(api, "getSession").mockResolvedValue(detail());
    vi.spyOn(api, "listKernels").mockResolvedValue([KERNEL]);
    vi.spyOn(api, "ensureTerminal").mockResolvedValue(RUNNING_TERMINAL);
    vi.spyOn(api, "getTerminalStatus").mockResolvedValue(RUNNING_TERMINAL);
    vi.spyOn(api, "getSessionTelemetry").mockResolvedValue(LIVE_TELEMETRY);
    vi.spyOn(api, "createSession").mockResolvedValue(CLI_SESSION);
    vi.spyOn(api, "resumeTerminal").mockResolvedValue({
        ...RUNNING_TERMINAL,
        attach_kind: "resumed",
    });
    vi.spyOn(api, "stopTerminal").mockResolvedValue({
        ...RUNNING_TERMINAL,
        state: "exited",
        exit_status: 0,
        attach_kind: null,
        attachment_count: 0,
    });
    vi.spyOn(api, "saveSessionWorkspace").mockResolvedValue({
        workspace_id: "saved",
        name: "Saved",
        status: "ready",
        mount_path: "/workspace/saved",
        volume_name: "saved-volume",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-01T00:00:00Z",
    });
    vi.spyOn(api, "deleteSession").mockResolvedValue();
    vi.stubGlobal("alert", vi.fn());
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

describe("CliView", () => {
    it("filters the agent picker and creates CLI-mode sessions", async () => {
        const onSelect = vi.fn();
        const user = userEvent.setup();
        render(
            <CliView darkMode={false} onSelectSession={onSelect} selectedSessionId={null} />,
            { wrapper: wrapper() },
        );

        await user.click(screen.getAllByRole("button", { name: "New CLI session" })[1]);
        const picker = await screen.findByRole("combobox", {
            name: "Agent",
            ...IN_DIALOG,
        });
        expect(screen.getByRole("option", { name: /CLI Agent/, ...IN_DIALOG })).toBeTruthy();
        expect(screen.queryByRole("option", {
            name: /Chat Only Agent/,
            ...IN_DIALOG,
        })).toBeNull();
        await user.selectOptions(picker, "cli-agent");
        await user.click(
            screen.getByRole("button", { name: "Start CLI session", ...IN_DIALOG }),
        );

        await waitFor(() => {
            expect(api.createSession).toHaveBeenCalledWith({
                agent_id: "cli-agent",
                channel_name: null,
                client_type: "webui",
                interaction_mode: "cli",
            });
        });
        expect(onSelect).toHaveBeenCalledWith(CLI_SESSION.session_id);
    });

    it("shows status, attach kind, IDs, attachment count, and browser-reachable VS Code", async () => {
        render(
            <CliView
                darkMode={false}
                onSelectSession={vi.fn()}
                selectedSessionId={CLI_SESSION.session_id}
            />,
            { wrapper: wrapper() },
        );

        expect(await screen.findByLabelText("Mock terminal")).toBeTruthy();
        expect(screen.getAllByText("live").length).toBeGreaterThanOrEqual(2);
        expect(screen.getByText("Started")).toBeTruthy();
        expect(screen.getByText("1 attachment")).toBeTruthy();
        expect(screen.getByText(/AgentSpace session-…/)).toBeTruthy();
        expect(screen.getByText(/Copilot copilot-…/)).toBeTruthy();
        expect(screen.getByRole("link", { name: "VS Code" }).getAttribute("href"))
            .toBe("http://localhost:8100/");
        expect(screen.getByLabelText("CLI telemetry summary")).toBeTruthy();
        expect(screen.getAllByText("48.4k tokens").length).toBeGreaterThan(0);
        expect(screen.getByText("16.5k input / 13 output")).toBeTruthy();
    });

    it("renders the usage strip and accessible details outside the terminal chrome", async () => {
        const user = userEvent.setup();
        render(
            <CliView
                darkMode={false}
                onSelectSession={vi.fn()}
                selectedSessionId={CLI_SESSION.session_id}
            />,
            { wrapper: wrapper() },
        );

        await screen.findByLabelText("Mock terminal");
        const summary = screen.getByLabelText("CLI telemetry summary");
        expect(summary.closest(".terminal-shell")).toBeNull();
        expect(summary.closest(".terminal-status")).toBeNull();
        expect(screen.getAllByText("99.6% (48k read)").length).toBeGreaterThan(0);
        expect(screen.getByText("17.8k / 272k")).toBeTruthy();
        expect(screen.getAllByText("1 subagent").length).toBeGreaterThan(0);

        await user.click(screen.getByRole("button", { name: "Usage details" }));
        expect(await screen.findByRole("dialog", { name: "CLI telemetry details" }))
            .toBeTruthy();
        expect(screen.getByText("Session usage")).toBeTruthy();
        expect(screen.getByText("Coverage & counts")).toBeTruthy();
        expect(screen.getByText("Health & policy")).toBeTruthy();
        expect(screen.getByText("Not reported by Copilot")).toBeTruthy();
        expect(screen.getByText("Metadata")).toBeTruthy();
    });

    it("shows unavailable telemetry without success-shaped zero defaults", async () => {
        vi.mocked(api.getSessionTelemetry).mockResolvedValue(UNAVAILABLE_TELEMETRY);
        const user = userEvent.setup();
        render(
            <CliView
                darkMode={false}
                onSelectSession={vi.fn()}
                selectedSessionId={CLI_SESSION.session_id}
            />,
            { wrapper: wrapper() },
        );
        await screen.findByLabelText("Mock terminal");
        const summary = screen.getByLabelText("CLI telemetry summary");
        await waitFor(() => {
            expect(summary.textContent).toContain("Not available");
        });
        expect(screen.queryByText("0 tokens")).toBeNull();
        expect(screen.queryByText("0% cache")).toBeNull();

        await user.click(screen.getByRole("button", { name: "Usage details" }));
        expect(await screen.findByText("Usage is unavailable for this session."))
            .toBeTruthy();
    });

    it("opens browser-local scrollback without mutating the shared tmux pane", async () => {
        const user = userEvent.setup();
        render(
            <CliView
                darkMode={false}
                onSelectSession={vi.fn()}
                selectedSessionId={CLI_SESSION.session_id}
            />,
            { wrapper: wrapper() },
        );
        await screen.findByLabelText("Mock terminal");
        await user.click(screen.getByRole("button", { name: "Scrollback" }));
        expect(screen.getByText(/Press/).textContent).toContain("q");
        expect(screen.getByText(/Browser-local scrollback/)).toBeTruthy();
    });

    it("keeps stop and delete distinct", async () => {
        const user = userEvent.setup();
        const confirm = vi.spyOn(window, "confirm");
        render(
            <CliView
                darkMode={false}
                onSelectSession={vi.fn()}
                selectedSessionId={CLI_SESSION.session_id}
            />,
            { wrapper: wrapper() },
        );
        await screen.findByLabelText("Mock terminal");

        await user.click(screen.getByRole("button", { name: "CLI session actions" }));
        await user.click(await screen.findByRole("menuitem", { name: "Stop CLI" }));
        await waitFor(() => {
            expect(api.stopTerminal).toHaveBeenCalledWith(CLI_SESSION.session_id);
        });
        expect(api.deleteSession).not.toHaveBeenCalled();

        confirm.mockReturnValueOnce(false).mockReturnValueOnce(true);
        await user.click(screen.getByRole("button", { name: "CLI session actions" }));
        await user.click(await screen.findByRole("menuitem", { name: "Delete session" }));
        await waitFor(() => {
            expect(api.deleteSession).toHaveBeenCalledWith(CLI_SESSION.session_id);
        });
        expect(api.stopTerminal).toHaveBeenCalledTimes(1);
    });

    it("detaches on navigation without stopping the CLI", async () => {
        const rendered = render(
            <CliView
                darkMode={false}
                onSelectSession={vi.fn()}
                selectedSessionId={CLI_SESSION.session_id}
            />,
            { wrapper: wrapper() },
        );
        await screen.findByLabelText("Mock terminal");
        rendered.unmount();
        expect(terminalMock.cleanup).toHaveBeenCalledWith(CLI_SESSION.session_id);
        expect(api.stopTerminal).not.toHaveBeenCalled();
    });

    it("resumes an exited CLI and reports the resumed attach kind", async () => {
        const exited = {
            ...RUNNING_TERMINAL,
            state: "exited" as const,
            exit_status: 7,
            attach_kind: null,
            attachment_count: 0,
        };
        terminalMock.attachKind = "resumed";
        vi.mocked(api.ensureTerminal).mockResolvedValueOnce(exited);
        let observedStatus: TerminalStatus = exited;
        vi.mocked(api.getTerminalStatus).mockImplementation(
            () => Promise.resolve(observedStatus),
        );
        vi.mocked(api.resumeTerminal).mockImplementation(() => {
            observedStatus = {
                ...RUNNING_TERMINAL,
                attach_kind: "resumed",
            };
            return Promise.resolve(observedStatus);
        });
        const user = userEvent.setup();
        render(
            <CliView
                darkMode={false}
                onSelectSession={vi.fn()}
                selectedSessionId={CLI_SESSION.session_id}
            />,
            { wrapper: wrapper() },
        );

        await user.click(await screen.findByRole("button", { name: "Resume Copilot" }));
        await waitFor(() => {
            expect(api.resumeTerminal).toHaveBeenCalledWith(CLI_SESSION.session_id);
        });
        expect(await screen.findByText("Resumed")).toBeTruthy();
    });
});

describe("SessionsView mode navigation", () => {
    it("routes Chat and CLI sessions to their matching views", async () => {
        const onChat = vi.fn();
        const onCli = vi.fn();
        const user = userEvent.setup();
        render(
            <SessionsView onNavigateToChat={onChat} onNavigateToCli={onCli} />,
            { wrapper: wrapper() },
        );

        const openButtons = await screen.findAllByRole("button", { name: "Open" });
        await user.click(openButtons[0]);
        await user.click(openButtons[1]);
        expect(onCli).toHaveBeenCalledWith(CLI_SESSION.session_id);
        expect(onChat).toHaveBeenCalledWith(CHAT_SESSION.session_id);
    });
});
