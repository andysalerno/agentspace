import { describe, expect, it } from "vitest";
import { buildMemoryTree, resolveMemoryLink } from "./memoryTree";
import type { MemoryPageSummary } from "./types";

function page(path: string): MemoryPageSummary {
  return {
    path,
    title: path.split("/").at(-1) ?? path,
    tags: [],
    updated_at: "2026-01-01T00:00:00Z",
  };
}

describe("buildMemoryTree", () => {
  it("groups nested pages and sorts folders before pages", () => {
    const tree = buildMemoryTree([
      page("z-page"),
      page("projects/beta"),
      page("projects/alpha"),
    ]);

    expect(tree.map((node) => node.name)).toEqual(["projects", "z-page"]);
    expect(tree[0].children.map((node) => node.name)).toEqual(["alpha", "beta"]);
    expect(tree[0].children[0].page?.path).toBe("projects/alpha");
  });
});

describe("resolveMemoryLink", () => {
  it("resolves relative Markdown page links without accepting external URLs", () => {
    expect(resolveMemoryLink("people/alice", "../projects/agentspace.md"))
      .toBe("projects/agentspace");
    expect(resolveMemoryLink("projects/notes/today", "./tomorrow.md#tasks"))
      .toBe("projects/notes/tomorrow");
    expect(resolveMemoryLink("people/alice", "https://example.com/page.md"))
      .toBeNull();
  });
});
