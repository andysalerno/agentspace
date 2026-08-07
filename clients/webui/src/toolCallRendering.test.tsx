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
    ["multi-line indented code", "Example:\n\n    line one\n    line two\n\nAfter."],
    ["a link with balanced parentheses", "See [docs](https://host/a_(b)) for details."],
    ["a code span across a line ending", "Run `foo\nbar` now."],
    ["a fence inside a block quote", "Note:\n\n> ```rust\n> fn main() {}\n> ```\n\nAfter."],
    ["a fence inside a list item", "Steps:\n\n- run it:\n\n  ```sh\n  cargo test\n  ```\n\nAfter."],
    ["an unterminated fence in a block quote", "Note:\n\n> ```rust\n> fn main() {"],
    ["an image", "Here ![alt text](https://host/i_(1).png) it is."],
    ["a nested code span in a link", "See [the `--all` flag](https://host/x) now."],
    ["a link reference", "See [the docs][ref] now.\n\n[ref]: https://host/x"],
    ["inline html", "Before <span title=\"a b\">middle</span> after."],
    ["backslash escapes", "Escaped \\* here and \\_more\\_ and \\\\ too."],
    ["character references", "Ten &amp; twenty &#39; and &#x2713; done."],
    ["a fence longer than its inner one", "Patch:\n\n````rust\nfn a() {}\n```\nstill code\n"],
    ["a fence before a heading", "Intro\n\n```\ncode\n```\n\n## Next\n\nBody."],
    ["a closed block before an open one", "Text.\n\n```\nclosed\n```\n\n```rust\nfn main() {"],
    ["an inert hash", "#heading text here"],
    ["an inert dash", "-not a list item"],
    ["an inert ordered marker", "1.not an ordered item"],
    ["an inert quote marker", ">not a quote here"],
    ["intraword emphasis markers", "a*b*c and d_e_f stay plain"],
    ["an unterminated html block", "<script>\nconst x = 1;"],
    ["an unterminated html block after text", "Before.\n\n<div>\nstuff here"],
    ["an over-indented closing fence", "```\ncode\n    ```"],
    ["windows line endings", "alpha\r\nbeta\r\ngamma"],
    ["windows line endings in a quote", "> alpha\r\n> beta"],
    ["a lone carriage return", "alpha\rbeta"],
];

/*
 * Documents for the ordering invariant below. Kept separate from
 * `trickyContent` because a document that cannot host a chip safely at some
 * offset legitimately relocates it -- "a*b*c" has to move the chip out of the
 * way rather than create emphasis that was not there -- and not corrupting
 * the message outranks placing it in stream order.
 */
const orderedContent: [string, string][] = [
    ["a closed block between paragraphs", "Intro.\n\n```\nx\n```\n\nAfter."],
    ["a trailing closed block", "Intro text here.\n\n```js\nconst x = 1;\n```"],
    ["an inline code span", "Alpha `code` omega"],
    ["a leading table", "| a | b |\n| - | - |\n| 1 | 2 |\n\nTail text."],
    ["a table between paragraphs", "Head.\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nTail."],
    ["a closed block before an open one", "One.\n\n```\nclosed\n```\n\n```rust\nfn main() {"],
    ["an image", "![img](http://h/i.png) after image"],
    ["a quote before a block", "> quote\n\n```\ncode\n```\n\nend"],
    ["carriage-return line endings", "```\rcode\r```"],
];

/**
 * The rendered document with the chips themselves taken back out.
 *
 * Whitespace is discarded from the comparisons this feeds: a chip is
 * deliberately padded so it cannot glue onto its neighbours, which splits a
 * word it lands inside. That padding is the intended behaviour, whereas any
 * change to the characters themselves is corruption.
 */
function withoutChips(container: HTMLElement): HTMLElement {
    const clone = container.cloneNode(true) as HTMLElement;
    for (const chip of clone.querySelectorAll(`a[href^="${toolCallHrefPrefix}"]`)) {
        chip.remove();
    }
    return clone;
}

function documentText(root: HTMLElement): string {
    return (root.textContent ?? "").replace(/\s+/g, "");
}

function structureOf(root: HTMLElement): string {
    return [...root.querySelectorAll("pre code, table, li, blockquote, img, a, h1, h2, h3")]
        .map((node) => `${node.tagName}:${documentText(node as HTMLElement)}`)
        .join("|");
}

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

    /*
     * The strongest guarantee this module owes: adding a chip may add the chip,
     * and nothing else. Rendering with and without one must produce the same
     * text and the same block structure, at every offset.
     */
    it.each(trickyContent)("leaves %s otherwise unchanged at every offset", (_name, content) => {
        const original = withoutChips(renderMessage(content, []).container);
        const expectedText = documentText(original);
        const expectedStructure = structureOf(original);

        for (let offset = 0; offset <= [...content].length; offset += 1) {
            const { container } = renderMessage(content, [{ tool: "grep", content_offset: offset }]);
            const stripped = withoutChips(container);
            expect(documentText(stripped), `offset ${offset}`).toBe(expectedText);
            expect(structureOf(stripped), `offset ${offset}`).toBe(expectedStructure);
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

    it("keeps a chip out of a heading", () => {
        const content = "Intro\n\n```\ncode\n```\n\n## Next\n\nBody.";
        const { container } = renderMessage(content, [{ tool: "grep", content_offset: 12 }]);
        expect(container.querySelector("h2")?.textContent).toBe("Next");
        expect(container.querySelector(`h2 a[href^="${toolCallHrefPrefix}"]`)).toBeNull();
    });

    it("does not park a chip ahead of a block that had already streamed", () => {
        // The chip belongs to the unclosed fence, so it must not jump back
        // over the closed block between it and the only paragraph of text.
        const content = "Text.\n\n```\nclosed\n```\n\n```rust\nfn main() {";
        const { container } = renderMessage(content, [{ tool: "edit", content_offset: 30 }]);
        const nodes = [...container.children];
        const chip = nodes.findIndex((node) => node.querySelector(`a[href^="${toolCallHrefPrefix}"]`));
        expect(chip).toBeGreaterThan(nodes.findIndex((node) => node.tagName === "PRE"));
        expect(container.querySelectorAll("pre")).toHaveLength(2);
        expect(nodes[0].textContent).toBe("Text.");
    });

    /*
     * Placement is chosen by rules, but guaranteed by re-parsing: whatever the
     * rules pick, a chip that would change the document is rejected in favour
     * of a position that cannot. These are the cases where the rules alone
     * reach for a position that does not survive that check.
     */
    it("keeps the chip visible when a block would otherwise swallow it", () => {
        for (const content of ["<script>\nconst x = 1;", "```\ncode\n    ```"]) {
            for (let offset = 0; offset <= [...content].length; offset += 1) {
                const { chipLabels } = renderMessage(content, [{ tool: "grep", content_offset: offset }]);
                expect(chipLabels, `${content} @ ${offset}`).toEqual(["⚙ grep"]);
            }
        }
    });

    /*
     * A tool call's offset is where it happened in the stream, so a chip must
     * never sit ahead of content that had already arrived. Checking that
     * placement only ever moves forward as the offset does catches that
     * whole class -- an offset at the very end of a code block, or in the gap
     * after one, used to anchor to text in front of the block while an offset
     * in the middle of it correctly went behind.
     */
    it.each(orderedContent)("places chips in offset order in %s", (_name, content) => {
        const chipLink = "[⚙ grep](#tool-call-0)";
        let previous = -1;
        for (let offset = 0; offset <= [...content].length; offset += 1) {
            const markdown = addInlineToolCalls(content, [{ tool: "grep", content_offset: offset }]);
            const at = markdown.indexOf(chipLink);
            expect(at, `offset ${offset} of ${JSON.stringify(content)}`).toBeGreaterThanOrEqual(previous);
            previous = at;
        }
    });

    it("keeps a chip behind a block that has already closed", () => {
        // The block finished streaming before the tool ran, so the chip
        // belongs after it even though there is no text there to anchor to.
        const content = "Intro text here.\n\n```js\nconst x = 1;\n```";
        const markdown = addInlineToolCalls(content, [{ tool: "grep", content_offset: 30 }]);
        expect(markdown.indexOf("[⚙ grep](#tool-call-0)")).toBeGreaterThan(markdown.indexOf("const x = 1;"));
    });

    it("does not split a CRLF into two line endings", () => {
        // remark-breaks renders a line ending as <br>, so a chip landing
        // between the CR and the LF would show up as a second break.
        const content = "alpha\r\nbeta";
        for (let offset = 0; offset <= [...content].length; offset += 1) {
            const { container } = renderMessage(content, [{ tool: "grep", content_offset: offset }]);
            expect(container.querySelectorAll("br"), `offset ${offset}`).toHaveLength(1);
        }
    });

    it("does not let the chip's own padding activate a block marker", () => {
        // The space that keeps a chip off its neighbours would turn the inert
        // "#" of this paragraph into a heading marker.
        const { container, text } = renderMessage("#heading text here", [
            { tool: "grep", content_offset: 1 },
        ]);
        expect(container.querySelector("h1")).toBeNull();
        expect(text).toContain("#heading text here");
    });

    it("does not let a tool title inject markdown into the message", () => {
        const { chipLabels, container } = renderMessage("Working.", [
            { tool: "](#) **injected** [x", content_offset: 8 },
        ]);
        expect(chipLabels).toEqual(["⚙ ](#) **injected** [x"]);
        expect(container.querySelector("strong")).toBeNull();
    });
});

/*
 * The hand-written cases above cover constructs one at a time. Real messages
 * nest them, and it is the combinations that tend to break a placement rule,
 * so compose documents out of fragments and check the same invariant. The
 * generator is seeded, so a failure names a document that can be replayed.
 */
const fragments = [
    "Plain sentence with `a span` in it.",
    "Trailing text.",
    "```rust\nfn main() { let x = (1, 2); }\n```",
    "> quoted line\n> more quoted",
    "- alpha `x`\n- beta [l](http://h/a_(b))",
    "1. first\n2. second",
    "## A heading `with code`",
    "| col | val |\n| --- | --- |\n| a | 1 |",
    "    indented code\n    second line",
    "See [docs](https://host/p_(1)/q) and ![img](https://host/i.png).",
    "Run `foo\nbar` across lines.",
    "> ```sh\n> cargo test\n> ```",
    "---",
    "Text with <span title=\"x y\">html</span> inline.",
    "- outer\n\n  ```js\n  const a = 1;\n  ```",
    "Escaped \\* and \\_ and &amp; and &#39; here.",
    "````\nouter ``` inner\n````",
    "#not a heading and -not a list.",
    "a*b*c and 1.not ordered.",
    "<script>\nconst x = 1;",
];

function seededRandom(seed: number): () => number {
    let state = seed >>> 0;
    return () => {
        state = (state * 1664525 + 1013904223) >>> 0;
        return state / 0x100000000;
    };
}

describe("inline tool call rendering (composed documents)", () => {
    it("leaves randomly composed markdown unchanged apart from the chip", () => {
        const random = seededRandom(20260807);
        for (let document = 0; document < 40; document += 1) {
            const count = 2 + Math.floor(random() * 3);
            const content = Array.from(
                { length: count },
                () => fragments[Math.floor(random() * fragments.length)],
            ).join("\n\n");

            const original = withoutChips(renderMessage(content, []).container);
            const expectedText = documentText(original);
            const expectedStructure = structureOf(original);
            const length = [...content].length;

            for (let offset = 0; offset <= length; offset += 5) {
                const { container, chipLabels } = renderMessage(content, [
                    { tool: "grep", content_offset: offset },
                ]);
                const where = `document ${document} offset ${offset}: ${JSON.stringify(content)}`;
                expect(chipLabels, where).toEqual(["⚙ grep"]);
                const stripped = withoutChips(container);
                expect(documentText(stripped), where).toBe(expectedText);
                expect(structureOf(stripped), where).toBe(expectedStructure);
            }
        }
    }, 30_000);
});
