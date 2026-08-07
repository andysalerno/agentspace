import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import ReactMarkdown from "react-markdown";
import {
    addInlineToolCalls,
    messageMarkdownPlugins,
    toolCallHrefPrefix,
} from "./toolCallMarkdown";
import type { ToolCall } from "./types";

/**
 * Render assistant content the way ChatView does and report what survived the
 * markdown pass: chips must come out as links, and none of the chip syntax may
 * leak into the visible message text.
 */
function renderMessage(content: string, toolCalls: ToolCall[]) {
    const { container } = render(
        <ReactMarkdown remarkPlugins={messageMarkdownPlugins}>
            {addInlineToolCalls(content, toolCalls)}
        </ReactMarkdown>,
    );
    const chips = [...container.querySelectorAll(`a[href^="${toolCallHrefPrefix}"]`)];
    return {
        chipLabels: chips.map((chip) => chip.textContent ?? ""),
        container,
        text: container.textContent ?? "",
    };
}

const trickyContent: [string, string][] = [
    ["a fenced code block", "Patch:\n\n```rust\nfn main() {}\n```\n\nDone."],
    ["an unterminated fence", "Patch:\n\n```rust\nfn main() {"],
    ["an inline code span", "Run `cargo test --all` to check."],
    ["a bulleted list", "Findings:\n\n- alpha\n- beta\n"],
    ["a numbered list", "Steps:\n\n1. alpha\n2. beta\n"],
    ["a heading", "Intro\n\n## Details\n\nBody text here."],
    ["a table", "| col | val |\n| --- | --- |\n| a | 1 |\n\nAfter the table."],
    ["a block quote", "Note:\n\n> quoted line\n> more quoted\n"],
    ["a markdown link", "See [the docs](https://example.com/a_b) for details."],
    ["astral characters", "Shipped 🎉 the fix 🚀 already"],
    ["a thematic break", "Before\n\n---\n\nAfter"],
    ["indented code", "Example:\n\n    indented code line\n\nAfter."],
];

describe("inline tool call rendering", () => {
    it.each(trickyContent)("keeps chip syntax out of the text of %s", (_name, content) => {
        for (let offset = 0; offset <= [...content].length; offset += 1) {
            const { chipLabels, text } = renderMessage(content, [
                { tool: "grep", content_offset: offset },
            ]);
            expect(chipLabels).toEqual(["⚙ grep"]);
            expect(text).not.toContain(toolCallHrefPrefix);
            expect(text).not.toContain("](");
        }
    });

    it("renders every chip of a multi-tool message as a link", () => {
        const content = "Looking.\n\n```\ncode\n```\n\n- item one\n- item two";
        const { chipLabels, text } = renderMessage(content, [
            { tool: "grep", content_offset: 0 },
            { tool: "view", content_offset: 12 },
            { tool: "edit", content_offset: [...content].length },
        ]);
        expect(chipLabels).toEqual(["⚙ grep", "⚙ view", "⚙ edit"]);
        expect(text).not.toContain(toolCallHrefPrefix);
    });

    it("keeps a code block intact when a chip lands inside it", () => {
        const content = "Patch:\n\n```rust\nfn main() {}\n```\n\nDone.";
        const { container } = renderMessage(content, [{ tool: "edit", content_offset: 20 }]);
        expect(container.querySelector("pre code")?.textContent).toBe("fn main() {}\n");
    });

    it("keeps a list intact when a chip lands on the list marker", () => {
        const content = "Findings:\n\n- alpha\n- beta";
        const { container } = renderMessage(content, [{ tool: "grep", content_offset: 11 }]);
        expect([...container.querySelectorAll("li")].map((item) => item.textContent)).toEqual([
            "alpha",
            "beta",
        ]);
    });

    it("keeps a table intact when a chip lands inside it", () => {
        const content = "| col | val |\n| --- | --- |\n| a | 1 |\n\nAfter.";
        const { container } = renderMessage(content, [{ tool: "view", content_offset: 20 }]);
        expect(container.querySelectorAll("table tbody tr")).toHaveLength(1);
    });

    it("renders a huge multi-line tool title as one short chip", () => {
        const tool = `bash\n${"echo hello world; ".repeat(40)}`;
        const { chipLabels } = renderMessage("Running the script.", [
            { tool, content_offset: 19 },
        ]);
        expect(chipLabels).toHaveLength(1);
        expect(chipLabels[0].length).toBeLessThanOrEqual(48);
        expect(chipLabels[0]).not.toContain("\n");
    });

    it("does not let a tool title inject markdown into the message", () => {
        const { chipLabels, container } = renderMessage("Working.", [
            { tool: "](#) **injected** [x", content_offset: 8 },
        ]);
        expect(chipLabels).toEqual(["⚙ ](#) **injected** [x"]);
        expect(container.querySelector("strong")).toBeNull();
    });
});
