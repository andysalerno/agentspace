import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, api } from "./api";
import MemoryView from "./MemoryView";
import type { MemoryPage } from "./types";

vi.mock("./CodeEditor", () => ({
  default: ({
    value,
    onChange,
  }: {
    value: string;
    onChange: (value: string) => void;
  }) => (
    <textarea
      aria-label="Memory body"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

const storedPage: MemoryPage = {
  path: "projects/agentspace",
  schema_version: 1,
  title: "AgentSpace",
  tags: ["project"],
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  created_by: "agent-session-one",
  updated_by: "agent-session-one",
  extra: {},
  revision: "revision-one",
  body: "Shared knowledge",
  outgoing_links: [],
};

function wrapper() {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  function TestWrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
  }
  return TestWrapper;
}

function mockHealthyMemory(pages: MemoryPage[] = [storedPage]) {
  vi.spyOn(api, "getMemoryHealth").mockResolvedValue({ status: "ok" });
  vi.spyOn(api, "listMemoryPages").mockResolvedValue(pages.map((page) => ({
    path: page.path,
    title: page.title,
    tags: page.tags,
    updated_at: page.updated_at,
  })));
  vi.spyOn(api, "listMemoryTags").mockResolvedValue([]);
  vi.spyOn(api, "getMemoryPage").mockImplementation((path) => {
    const page = pages.find((candidate) => candidate.path === path);
    if (!page) throw new ApiError("not found", 404, undefined);
    return Promise.resolve(page);
  });
  vi.spyOn(api, "getMemoryLinks").mockResolvedValue({
    path: storedPage.path,
    outgoing: [],
    backlinks: [],
  });
  vi.spyOn(api, "checkMemory").mockResolvedValue({ issues: [] });
}

beforeEach(() => {
  vi.stubGlobal("confirm", vi.fn(() => true));
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("MemoryView", () => {
  it("distinguishes a healthy empty store from an unavailable service", async () => {
    mockHealthyMemory([]);
    const rendered = render(<MemoryView />, { wrapper: wrapper() });
    expect(await screen.findByText(/The memory store is empty/)).toBeTruthy();
    rendered.unmount();

    vi.restoreAllMocks();
    vi.spyOn(api, "getMemoryHealth").mockRejectedValue(
      new ApiError("unavailable", 503, undefined),
    );
    vi.spyOn(api, "listMemoryPages").mockRejectedValue(
      new ApiError("unavailable", 503, undefined),
    );
    vi.spyOn(api, "listMemoryTags").mockRejectedValue(
      new ApiError("unavailable", 503, undefined),
    );
    vi.spyOn(api, "checkMemory").mockRejectedValue(
      new ApiError("unavailable", 503, undefined),
    );

    render(<MemoryView />, { wrapper: wrapper() });
    expect(await screen.findByText("Memory service unavailable")).toBeTruthy();
  });

  it("guards browser saves with the loaded revision and preserves stale drafts", async () => {
    mockHealthyMemory();
    const conflict = new ApiError(
      "page changed",
      409,
      {
        error: {
          kind: "conflict",
          message: "page changed",
          path: storedPage.path,
          expected_revision: "revision-one",
          actual_revision: "revision-two",
        },
      },
    );
    const write = vi.spyOn(api, "writeMemoryPage")
      .mockRejectedValueOnce(conflict)
      .mockResolvedValue({
        ...storedPage,
        path: "projects/agentspace-draft",
        revision: "draft-revision",
      });
    vi.stubGlobal("prompt", vi.fn(() => "projects/agentspace-draft"));
    const user = userEvent.setup();

    render(<MemoryView />, { wrapper: wrapper() });
    const title = await screen.findByLabelText("Title");
    await user.clear(title);
    await user.type(title, "My stale draft");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(write).toHaveBeenCalledWith(
        storedPage.path,
        expect.objectContaining({
          title: "My stale draft",
          expected_revision: "revision-one",
        }),
      );
    });
    expect(await screen.findByText("This page changed after you opened it."))
      .toBeTruthy();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value)
      .toBe("My stale draft");
    expect(screen.getByRole("button", { name: "Save" }).hasAttribute("disabled"))
      .toBe(true);
    expect(screen.getByRole("button", { name: "Save my draft as a new page" }))
      .toBeTruthy();

    fireEvent.click(
      screen.getByRole("button", { name: "Save my draft as a new page" }),
    );
    await waitFor(() => {
      expect(write).toHaveBeenLastCalledWith(
        "projects/agentspace-draft",
        expect.objectContaining({
          body: storedPage.body,
          overwrite: false,
        }),
      );
    });
  });
});
