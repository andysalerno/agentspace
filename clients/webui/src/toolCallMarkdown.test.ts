import { describe, expect, it } from "vitest";
import {
    addInlineToolCalls,
    characterLength,
    toolCallLabel,
    toolCallTooltip,
} from "./toolCallMarkdown";
import type { ToolCall } from "./types";

function toolCall(tool: string, contentOffset?: number): ToolCall {
    return { tool, content_offset: contentOffset };
}

/** The chip link markdown the renderer swaps for a button. */
function chip(label: string, index = 0): string {
    return `[⚙ ${label}](#tool-call-${index})`;
}

describe("toolCallLabel", () => {
    it("collapses multi-line titles onto one line", () => {
        expect(toolCallLabel("bash\ncurl -s https://x \\\n  | jq .")).toBe(
            "bash curl -s https://x \\ | jq .",
        );
    });

    it("truncates long titles", () => {
        const label = toolCallLabel("x".repeat(500));
        expect(label).toHaveLength(41);
        expect(label.endsWith("…")).toBe(true);
    });

    it("does not split surrogate pairs while truncating", () => {
        const label = toolCallLabel("🎉".repeat(80));
        expect([...label]).toHaveLength(41);
        expect(label).not.toContain("\uFFFD");
    });

    it("falls back to a placeholder for blank titles", () => {
        expect(toolCallLabel("   \n  ")).toBe("tool");
    });
});

describe("toolCallTooltip", () => {
    it("keeps more of the title than the chip but stays bounded", () => {
        expect(toolCallTooltip("view\nsrc/main.rs")).toBe("view src/main.rs");
        expect([...toolCallTooltip("y".repeat(1000))]).toHaveLength(401);
    });
});

describe("characterLength", () => {
    it("counts astral characters once, like the server does", () => {
        expect(characterLength("done 🎉")).toBe(6);
        expect("done 🎉").toHaveLength(7);
    });
});

describe("addInlineToolCalls", () => {
    it("returns the content untouched when there are no tool calls", () => {
        expect(addInlineToolCalls("hello", [])).toBe("hello");
    });

    it("places a chip inline at a mid-paragraph offset", () => {
        expect(addInlineToolCalls("Reading the file. Found it.", [toolCall("view", 17)])).toBe(
            `Reading the file. ${chip("view")} Found it.`,
        );
    });

    it("escapes markdown syntax in the label so the chip cannot break out", () => {
        expect(addInlineToolCalls("ok", [toolCall("read [a](b) *x*", 2)])).toBe(
            `ok ${chip("read \\[a\\]\\(b\\) \\*x\\*")}`,
        );
    });

    it("reads offsets as character counts, not UTF-16 code units", () => {
        // "Shipped 🎉" is 9 characters but 10 UTF-16 code units.
        expect(addInlineToolCalls("Shipped 🎉 now", [toolCall("view", 9)])).toBe(
            `Shipped 🎉 ${chip("view")} now`,
        );
    });

    it("moves a chip out of a fenced code block", () => {
        const content = "Patch:\n\n```rust\nfn main() {}\n```\n\nDone.";
        expect(addInlineToolCalls(content, [toolCall("edit", 20)])).toBe(
            `Patch:\n\n\`\`\`rust\nfn main() {}\n\`\`\`\n\n${chip("edit")} Done.`,
        );
    });

    it("places a chip before a fence that has not finished streaming", () => {
        const content = "Patch:\n\n```rust\nfn main() {}";
        expect(addInlineToolCalls(content, [toolCall("edit", 20)])).toBe(
            `Patch: ${chip("edit")}\n\n\`\`\`rust\nfn main() {}`,
        );
    });

    it("moves a chip out of an inline code span", () => {
        expect(addInlineToolCalls("Run `cargo test` now", [toolCall("bash", 8)])).toBe(
            `Run \`cargo test\` ${chip("bash")} now`,
        );
    });

    it("moves a chip out of a markdown link", () => {
        expect(addInlineToolCalls("See [the docs](http://x/y) here", [toolCall("fetch", 8)])).toBe(
            `See [the docs](http://x/y) ${chip("fetch")} here`,
        );
    });

    it("does not displace a list marker", () => {
        const content = "Findings:\n\n- first\n- second";
        expect(addInlineToolCalls(content, [toolCall("grep", 11)])).toBe(
            `Findings: ${chip("grep")}\n\n- first\n- second`,
        );
    });

    it("does not displace a heading", () => {
        const content = "Summary\n\n## Details\n\nBody.";
        expect(addInlineToolCalls(content, [toolCall("grep", 9)])).toBe(
            `Summary ${chip("grep")}\n\n## Details\n\nBody.`,
        );
    });

    it("does not turn a following fence into inline text", () => {
        const content = "Here:\n\n```\ncode\n```";
        expect(addInlineToolCalls(content, [toolCall("view", 7)])).toBe(
            `Here: ${chip("view")}\n\n\`\`\`\ncode\n\`\`\``,
        );
    });

    it("renders a chip as its own paragraph when no line can host it", () => {
        const content = "```\ncode\n```\n\ntail";
        expect(addInlineToolCalls(content, [toolCall("view", 0)])).toBe(
            `${chip("view")}\n\n\`\`\`\ncode\n\`\`\`\n\ntail`,
        );
    });

    it("keeps a chip out of a table", () => {
        const content = "| a | b |\n| - | - |\n| 1 | 2 |\n\ntail";
        expect(addInlineToolCalls(content, [toolCall("view", 15)])).toBe(
            `| a | b |\n| - | - |\n| 1 | 2 |\n\n${chip("view")} tail`,
        );
    });

    it("places leading chips inline ahead of plain text", () => {
        expect(addInlineToolCalls("Found it.", [toolCall("view", 0), toolCall("grep", 0)])).toBe(
            `${chip("view")} ${chip("grep", 1)} Found it.`,
        );
    });

    it("places chips inline when there is no content at all", () => {
        expect(addInlineToolCalls("", [toolCall("view"), toolCall("grep")])).toBe(
            `${chip("view")} ${chip("grep", 1)}`,
        );
    });

    it("keeps chips in offset order and clamps out-of-range offsets", () => {
        const content = "one two";
        expect(addInlineToolCalls(content, [toolCall("b", 9999), toolCall("a", 3)])).toBe(
            `one ${chip("a", 1)} two ${chip("b")}`,
        );
    });

    it("ignores a missing or non-finite offset", () => {
        expect(addInlineToolCalls("text", [toolCall("view", Number.NaN)])).toBe(
            `${chip("view")} text`,
        );
    });
});
