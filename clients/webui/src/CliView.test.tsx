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
            return <div aria-label="Mock terminal">connected terminal</div>;
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
        expect(screen.getAllByText("live")).toHaveLength(2);
        expect(screen.getByText("Started")).toBeTruthy();
        expect(screen.getByText("1 attachment")).toBeTruthy();
        expect(screen.getByText(/AgentSpace session-…/)).toBeTruthy();
        expect(screen.getByText(/Copilot copilot-…/)).toBeTruthy();
        expect(screen.getByRole("link", { name: "VS Code" }).getAttribute("href"))
            .toBe("http://localhost:8100/");
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
