import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "./api";
import ConnectionsView from "./ConnectionsView";
import { ErrorProvider } from "./ErrorContext";
import type { Connection, SecretStatus } from "./types";

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

const SECRETS: SecretStatus[] = [
  { name: "OPENAI_API_KEY", description: null, is_set: true, references: [] },
  { name: "UNSET_KEY", description: null, is_set: false, references: [] },
];

const CONNECTION: Connection = {
  connection_id: "openai",
  name: "OpenAI",
  url: "https://api.openai.com/v1",
  api_flavor: "chat_completions",
  has_api_key: true,
  api_key_secret: "OPENAI_API_KEY",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("ConnectionsView", () => {
  it("shows the referenced secret name instead of a key value", async () => {
    vi.spyOn(api, "listConnections").mockResolvedValue([CONNECTION]);
    vi.spyOn(api, "listSecrets").mockResolvedValue(SECRETS);
    render(<ConnectionsView />, { wrapper: wrapper() });

    expect(await screen.findByText("OPENAI_API_KEY")).toBeTruthy();
  });

  it("creates a connection referencing a declared secret by name", async () => {
    vi.spyOn(api, "listConnections").mockResolvedValue([]);
    vi.spyOn(api, "listSecrets").mockResolvedValue(SECRETS);
    const create = vi.spyOn(api, "createConnection").mockResolvedValue(CONNECTION);
    const user = userEvent.setup();
    render(<ConnectionsView />, { wrapper: wrapper() });

    await user.click(screen.getAllByRole("button", { name: "New connection" })[0]);
    // Typed input is used sparingly here: tabster's modal focus trap blurs the
    // active element under jsdom, which drops keystrokes.
    fireEvent.change(screen.getByLabelText(/Connection ID/), {
      target: { value: "openai" },
    });
    fireEvent.change(screen.getByLabelText(/Display name/), {
      target: { value: "OpenAI" },
    });
    fireEvent.change(screen.getByLabelText(/Endpoint URL/), {
      target: { value: "https://api.openai.com/v1" },
    });

    // Unset declarations remain selectable so a connection can be wired up
    // before its value is installed.
    const picker = await screen.findByLabelText(/API key secret/);
    expect(
      screen.getByRole("option", { name: "UNSET_KEY (value not set)" }),
    ).toBeTruthy();
    await user.selectOptions(picker, "OPENAI_API_KEY");
    await user.click(screen.getByRole("button", { name: "Create connection" }));

    await waitFor(() => {
      expect(create).toHaveBeenCalledWith({
        connection_id: "openai",
        name: "OpenAI",
        url: "https://api.openai.com/v1",
        api_flavor: "chat_completions",
        api_key_secret: "OPENAI_API_KEY",
      });
    });
  });

  it("keeps a YAML-authored literal key when an unrelated field is edited", async () => {
    const literalBacked: Connection = {
      ...CONNECTION,
      has_api_key: true,
      api_key_secret: null,
    };
    vi.spyOn(api, "listConnections").mockResolvedValue([literalBacked]);
    vi.spyOn(api, "listSecrets").mockResolvedValue(SECRETS);
    const update = vi.spyOn(api, "updateConnection").mockResolvedValue(literalBacked);
    const user = userEvent.setup();
    render(<ConnectionsView />, { wrapper: wrapper() });

    await user.click(await screen.findByRole("button", { name: "Edit" }));
    const name = screen.getByLabelText(/Display name/);
    fireEvent.change(name, { target: { value: "Renamed" } });
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    // The literal is not representable in the picker, so the field is omitted
    // rather than sent as a clear.
    await waitFor(() => {
      expect(update).toHaveBeenCalledWith("openai", {
        name: "Renamed",
        url: "https://api.openai.com/v1",
        api_flavor: "chat_completions",
      });
    });
  });

  it("clears a YAML-authored literal only when explicitly deselected", async () => {
    const literalBacked: Connection = {
      ...CONNECTION,
      has_api_key: true,
      api_key_secret: null,
    };
    vi.spyOn(api, "listConnections").mockResolvedValue([literalBacked]);
    vi.spyOn(api, "listSecrets").mockResolvedValue(SECRETS);
    const update = vi.spyOn(api, "updateConnection").mockResolvedValue(literalBacked);
    const user = userEvent.setup();
    render(<ConnectionsView />, { wrapper: wrapper() });

    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.selectOptions(await screen.findByLabelText(/API key secret/), "");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(update).toHaveBeenCalledWith("openai", {
        name: "OpenAI",
        url: "https://api.openai.com/v1",
        api_flavor: "chat_completions",
        api_key_secret: "",
      });
    });
  });

  it("clears the reference when no secret is selected", async () => {
    vi.spyOn(api, "listConnections").mockResolvedValue([CONNECTION]);
    vi.spyOn(api, "listSecrets").mockResolvedValue(SECRETS);
    const update = vi.spyOn(api, "updateConnection").mockResolvedValue(CONNECTION);
    const user = userEvent.setup();
    render(<ConnectionsView />, { wrapper: wrapper() });

    await user.click(await screen.findByRole("button", { name: "Edit" }));
    await user.selectOptions(await screen.findByLabelText(/API key secret/), "");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => {
      expect(update).toHaveBeenCalledWith("openai", {
        name: "OpenAI",
        url: "https://api.openai.com/v1",
        api_flavor: "chat_completions",
        api_key_secret: "",
      });
    });
  });
});
