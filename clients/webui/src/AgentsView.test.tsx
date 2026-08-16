import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import AgentsView from "./AgentsView";
import { api } from "./api";
import { IN_DIALOG } from "./dialogTestQuery";
import { ErrorProvider } from "./ErrorContext";
import { FluentProvider } from "./fluent";
import { lightTheme } from "./theme";
import type { Agent, Connection } from "./types";

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

const CONNECTION: Connection = {
  connection_id: "openrouter",
  name: "OpenRouter",
  url: "https://openrouter.ai/api/v1",
  api_flavor: "responses",
  has_api_key: true,
  api_key_secret: null,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

const AGENT: Agent = {
  agent_id: "reviewer",
  name: "Reviewer",
  harness: "acp",
  system_prompt: "Review this workspace.",
  skills: [],
  env_vars: "",
  connection_id: null,
  cli: {
    harness: "copilot-cli",
    connection_id: "openrouter",
  },
  workspace_mounts: [],
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("AgentsView CLI capability", () => {
  it("displays and updates the nested CLI configuration", async () => {
    vi.spyOn(api, "listAgents").mockResolvedValue([AGENT]);
    vi.spyOn(api, "listConnections").mockResolvedValue([CONNECTION]);
    vi.spyOn(api, "listHarnesses").mockResolvedValue(["acp", "copilot-cli"]);
    vi.spyOn(api, "listSessions").mockResolvedValue([]);
    vi.spyOn(api, "listSkills").mockResolvedValue([]);
    vi.spyOn(api, "listWorkspaces").mockResolvedValue([]);
    vi.spyOn(api, "listConnectionModels").mockResolvedValue({ data: [] });
    vi.spyOn(api, "getKernelConfig").mockResolvedValue({
      harness: "copilot-cli",
      env_vars: "",
      updated_at: null,
    });
    const create = vi.spyOn(api, "createAgent").mockResolvedValue(AGENT);
    const user = userEvent.setup();

    render(<AgentsView onSessionCreated={vi.fn()} />, { wrapper: wrapper() });

    expect(await screen.findByText("Copilot CLI")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "New agent" }));
    fireEvent.change(await screen.findByLabelText(/Agent ID/), {
      target: { value: "new-reviewer" },
    });
    fireEvent.change(screen.getByLabelText(/Display name/), {
      target: { value: "New Reviewer" },
    });
    const enabled = await screen.findByLabelText("Enable CLI sessions");
    expect((enabled as HTMLInputElement).checked).toBe(false);
    await user.click(enabled);
    await user.selectOptions(screen.getByLabelText("CLI connection"), "openrouter");
    await user.click(
      await screen.findByRole("button", { name: "Create agent", ...IN_DIALOG }),
    );

    await waitFor(() => {
      expect(create).toHaveBeenCalledWith(expect.objectContaining({
        agent_id: "new-reviewer",
        name: "New Reviewer",
        cli: {
          harness: "copilot-cli",
          connection_id: "openrouter",
        },
      }));
    });
  });
});
