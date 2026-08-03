import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "./api";
import { IN_DIALOG } from "./dialogTestQuery";
import ConfigurationView from "./ConfigurationView";
import { ErrorProvider } from "./ErrorContext";
import SecretsView from "./SecretsView";

vi.mock("./CodeEditor", () => ({
  default: ({
    value,
    onChange,
  }: {
    value: string;
    onChange: (value: string) => void;
  }) => (
    <textarea
      aria-label="YAML configuration"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

function wrapper() {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  function TestWrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>
        <ErrorProvider>{children}</ErrorProvider>
      </QueryClientProvider>
    );
  }
  return TestWrapper;
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("ConfigurationView", () => {
  it("loads the active canonical config into the editor", async () => {
    vi.spyOn(api, "getCanonicalConfig").mockResolvedValue(
      "kind: AgentSpaceConfig\nspec:\n  agents: []\n",
    );
    render(<ConfigurationView />, { wrapper: wrapper() });

    const editor = await screen.findByLabelText<HTMLTextAreaElement>(
      "YAML configuration",
    );
    await waitFor(() => {
      expect(editor.value).toBe("kind: AgentSpaceConfig\nspec:\n  agents: []\n");
    });
  });

  it("validates and applies the exact editor source", async () => {
    vi.spyOn(api, "getCanonicalConfig").mockResolvedValue("kind: Active\n");
    const validate = vi.spyOn(api, "validateConfig").mockResolvedValue({ valid: true });
    vi.spyOn(api, "planConfig").mockResolvedValue({ active_generation: 7 });
    const apply = vi.spyOn(api, "applyConfig").mockResolvedValue({ generation: 2 });
    const user = userEvent.setup();
    render(<ConfigurationView />, { wrapper: wrapper() });

    const editor = await screen.findByLabelText<HTMLTextAreaElement>(
      "YAML configuration",
    );
    await waitFor(() => {
      expect(editor.value).toBe("kind: Active\n");
    });
    fireEvent.change(editor, { target: { value: "kind: AgentSpaceConfig\n" } });
    await user.click(screen.getByRole("button", { name: "Validate" }));
    await waitFor(() => {
      expect(validate).toHaveBeenCalledWith("kind: AgentSpaceConfig\n");
    });

    await user.click(screen.getByRole("button", { name: "Preview replacement" }));
    expect(await screen.findByText("Applying against generation 7")).toBeTruthy();
    await user.click(
      await screen.findByRole("button", { name: "Apply replacement" }),
    );
    await waitFor(() => {
      expect(apply).toHaveBeenCalledWith("kind: AgentSpaceConfig\n", 7);
    });
    expect(await screen.findByText(/"generation": 2/)).toBeTruthy();
  });
});

describe("SecretsView", () => {
  it("sets a value without ever fetching or rendering it", async () => {
    vi.spyOn(api, "listSecrets").mockResolvedValue([
      {
        name: "TOKEN",
        description: "Service token",
        is_set: false,
        references: ["connections/primary/apiKey"],
      },
    ]);
    const setValue = vi.spyOn(api, "setSecretValue").mockResolvedValue();
    const user = userEvent.setup();
    render(<SecretsView />, { wrapper: wrapper() });

    await user.click(await screen.findByRole("button", { name: "Set value" }));
    const input = await screen.findByPlaceholderText("Value is never displayed");
    fireEvent.change(input, { target: { value: "hidden-value" } });
    await user.click(
      await screen.findByRole("button", { name: "Save value", ...IN_DIALOG }),
    );

    await waitFor(() => {
      expect(setValue).toHaveBeenCalledWith("TOKEN", "hidden-value");
    });
    expect(screen.queryByText("hidden-value")).toBeNull();
  });
});
