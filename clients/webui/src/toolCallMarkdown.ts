/*
 * Inline tool-call chips for assistant markdown.
 *
 * Tool calls are recorded with a `content_offset` — the number of characters of
 * assistant text that had streamed when the call started. To draw a chip at
 * that point the offset is turned into a markdown link that the renderer swaps
 * for a button.
 *
 * Splicing text into markdown is only safe at some positions, and deciding
 * which ones by pattern-matching the source does not work: code spans and
 * fences may be nested in block quotes or list items, link destinations may
 * contain balanced parentheses, and a code span may run across a line ending.
 * So the content is parsed with the same remark pipeline that renders it and
 * every decision is made against the resulting syntax tree. Offsets inside a
 * construct that a chip would corrupt are relocated to the nearest position
 * that parses identically with the chip present, and labels are collapsed,
 * truncated and escaped so a long or multi-line tool title can neither blow up
 * the chip nor break out of it.
 */
import type { Root } from "mdast";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import { unified } from "unified";
import type { ToolCall } from "./types";

/** Remark plugins every assistant/user message is rendered with. */
export const messageMarkdownPlugins = [remarkGfm, remarkBreaks];

/** Href scheme that smuggles a chip through the markdown renderer. */
export const toolCallHrefPrefix = "#tool-call-";

/** Chip labels beyond this many characters are elided. */
const maxLabelCharacters = 40;

/** Native tooltips carry more of the title than the chip, but not all of it. */
const maxTooltipCharacters = 400;

const asciiPunctuation = /[!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~]/g;

/**
 * Constructs whose interior a chip cannot be spliced into.
 *
 * Some would swallow the chip and render it as literal text (code, math), some
 * would be broken by it (links and images, whose text cannot nest a link), and
 * some would be restructured by it (tables, whose cells are delimited by the
 * source layout). Chips landing inside any of them move to a boundary instead.
 */
const protectedTypes = new Set([
    "code",
    "definition",
    "footnoteDefinition",
    "footnoteReference",
    "html",
    "image",
    "imageReference",
    "inlineCode",
    "inlineMath",
    "link",
    "linkReference",
    "math",
    "table",
    "thematicBreak",
    "yaml",
]);

/** A half-open source range, in UTF-16 code units. */
type Span = { start: number; end: number };

type ProtectedRegion = Span & {
    /**
     * Set on a fenced block that runs to the end of the content without a
     * closing fence, i.e. one that is still streaming. It has no end to escape
     * past yet, so chips inside it go in front of the block.
     */
    unclosed: boolean;
    /** Start of the top-level block this region belongs to. */
    blockStart: number;
    /** End of the top-level block this region belongs to. */
    blockEnd: number;
};

/** The parsed shape of one message, reused across every chip in that message. */
type Document = {
    /** Ranges of plain text a chip may be spliced into at any position. */
    anchors: Span[];
    /** Top-level blocks, in source order. */
    blocks: Span[];
    regions: ProtectedRegion[];
};

type Insertion = { at: number; prefix: string; suffix: string };

type SyntaxNode = {
    type: string;
    position?: {
        start: { offset?: number | undefined };
        end: { offset?: number | undefined };
    };
    children?: SyntaxNode[];
};

/*
 * Only the parse step matters here, so the pipeline carries the parser and the
 * extensions that change how source is parsed. `remark-breaks` is a tree
 * transform rather than a parser extension, so it has no bearing on offsets.
 */
const parser = unified().use(remarkParse).use(remarkGfm);

/**
 * Count Unicode scalar values rather than UTF-16 code units.
 *
 * The server records `content_offset` with Rust's `chars().count()`, so
 * astral-plane characters (emoji, most notably) count as one. Measuring
 * locally-streamed content the same way keeps both sources of offsets in the
 * same coordinate space.
 */
export function characterLength(value: string): number {
    return [...value].length;
}

/** A single-line, length-bounded chip label for a possibly huge tool title. */
export function toolCallLabel(tool: string): string {
    const collapsed = tool.replace(/\s+/g, " ").trim();
    if (collapsed === "") {
        return "tool";
    }
    const characters = [...collapsed];
    if (characters.length <= maxLabelCharacters) {
        return collapsed;
    }
    return `${characters.slice(0, maxLabelCharacters).join("").trimEnd()}…`;
}

/** The chip's `title` attribute: more of the tool title, still bounded. */
export function toolCallTooltip(tool: string): string {
    const collapsed = tool.replace(/\s+/g, " ").trim();
    const characters = [...collapsed];
    if (characters.length <= maxTooltipCharacters) {
        return collapsed;
    }
    return `${characters.slice(0, maxTooltipCharacters).join("").trimEnd()}…`;
}

/** Escape every ASCII punctuation character so link text stays link text. */
function escapeMarkdownText(value: string): string {
    return value.replace(asciiPunctuation, "\\$&");
}

function toolCallLink(toolCall: ToolCall, index: number): string {
    return `[⚙ ${escapeMarkdownText(toolCallLabel(toolCall.tool))}](${toolCallHrefPrefix}${index})`;
}

function spanOf(node: SyntaxNode): Span | null {
    const start = node.position?.start.offset;
    const end = node.position?.end.offset;
    if (start === undefined || end === undefined || end <= start) {
        return null;
    }
    return { start, end };
}

/** Indentation and block quote markers that a container repeats on each line. */
const containerPrefix = /^(?:[ \t]*>)*[ \t]*/;

/**
 * Split a text node into the ranges a chip may actually be spliced into.
 *
 * A text node spans its source verbatim, so when it runs over a line ending
 * inside a block quote or a list it also covers the `>` markers and the
 * indentation that continue the container on the next line. Splicing a chip
 * into one of those would drop the line out of its container, so each
 * continuation line contributes an anchor that starts after its prefix.
 */
function textAnchors(content: string, span: Span): Span[] {
    const anchors: Span[] = [];
    let lineStart = span.start;
    while (lineStart < span.end) {
        const newline = content.indexOf("\n", lineStart);
        const lineEnd = newline < 0 || newline > span.end ? span.end : newline;
        const start =
            lineStart === span.start
                ? lineStart
                : lineStart + (containerPrefix.exec(content.slice(lineStart, lineEnd))?.[0].length ?? 0);
        if (start < lineEnd) {
            anchors.push({ start, end: lineEnd });
        }
        if (newline < 0) {
            break;
        }
        lineStart = newline + 1;
    }
    return anchors;
}

/** Strip the block quote markers and indentation a container adds to a line. */
function stripContainerPrefix(line: string): string {
    return line.replace(/^[ \t]*(?:>[ \t]?)*/, "");
}

/**
 * Whether a fenced block never closes, which means the message is still
 * streaming it and everything after the opening fence is provisional.
 */
function isUnclosedFence(content: string, span: Span): boolean {
    if (span.end < content.length) {
        return false;
    }
    const lines = content.slice(span.start, span.end).split("\n");
    const opening = /^(`{3,}|~{3,})/.exec(stripContainerPrefix(lines[0]));
    if (!opening) {
        return false;
    }
    const closing = new RegExp(`^\\${opening[1][0]}{3,}[ \t]*$`);
    return !lines.slice(1).some((line) => closing.test(stripContainerPrefix(line)));
}

function parseDocument(content: string): Document {
    const tree: Root = parser.parse(content);
    const anchors: Span[] = [];
    const regions: ProtectedRegion[] = [];
    const blocks: Span[] = [];

    for (const child of tree.children) {
        const block = spanOf(child);
        if (!block) {
            continue;
        }
        blocks.push(block);

        const stack: { node: SyntaxNode; guarded: boolean }[] = [{ node: child, guarded: false }];
        while (stack.length > 0) {
            const entry = stack.pop();
            if (!entry) {
                break;
            }
            const { node, guarded } = entry;
            const span = spanOf(node);
            const isProtected = protectedTypes.has(node.type);

            if (span && !guarded) {
                if (isProtected) {
                    regions.push({
                        ...span,
                        unclosed: node.type === "code" && isUnclosedFence(content, span),
                        blockStart: block.start,
                        blockEnd: block.end,
                    });
                } else if (node.type === "text") {
                    anchors.push(...textAnchors(content, span));
                }
            }
            for (const nested of node.children ?? []) {
                stack.push({ node: nested, guarded: guarded || isProtected });
            }
        }
    }

    anchors.sort((left, right) => left.start - right.start);
    blocks.sort((left, right) => left.start - right.start);
    return { anchors, blocks, regions };
}

/**
 * The outermost protected region strictly containing `offset`.
 *
 * An offset exactly on a region boundary is left alone: a chip there sits
 * beside the construct rather than inside it.
 */
function enclosingRegion(doc: Document, offset: number): ProtectedRegion | null {
    let found: ProtectedRegion | null = null;
    for (const region of doc.regions) {
        if (region.start >= offset || offset >= region.end) {
            continue;
        }
        if (!found || region.end - region.start > found.end - found.start) {
            found = region;
        }
    }
    return found;
}

/** The latest position at or before `offset` that a chip may be spliced into. */
function anchorAtOrBefore(doc: Document, offset: number): number | null {
    let best: number | null = null;
    for (const anchor of doc.anchors) {
        if (anchor.start > offset) {
            break;
        }
        const candidate = Math.min(offset, anchor.end);
        if (best === null || candidate > best) {
            best = candidate;
        }
    }
    return best;
}

/** The earliest position at or after `offset` that a chip may be spliced into. */
function anchorAtOrAfter(doc: Document, offset: number): number | null {
    for (const anchor of doc.anchors) {
        if (anchor.end >= offset) {
            return Math.max(offset, anchor.start);
        }
    }
    return null;
}

function blockAt(doc: Document, offset: number): Span | null {
    let previous: Span | null = null;
    for (const block of doc.blocks) {
        if (offset <= block.end) {
            return block.start <= offset ? block : (previous ?? block);
        }
        previous = block;
    }
    return previous;
}

function spacedInsertion(content: string, at: number): Insertion {
    const before = content.charAt(at - 1);
    const after = content.charAt(at);
    return {
        at,
        // A chip pressed against neighbouring text would glue onto it, and a
        // preceding `!` would even turn the chip's link into an image.
        prefix: at > 0 && before !== "" && !/\s/.test(before) ? " " : "",
        suffix: after !== "" && !/\s/.test(after) ? " " : "",
    };
}

function ownParagraph(at: number): Insertion {
    return { at, prefix: at > 0 ? "\n\n" : "", suffix: "\n\n" };
}

/**
 * Move a raw offset to the closest position where a chip can be spliced in
 * without changing how the surrounding markdown parses.
 *
 * Escaping a protected construct has a direction: a chip pushed out the back
 * of a code block stays behind it, while one pushed out of a fence that is
 * still streaming goes in front of the whole block. Everywhere else the chip
 * attaches to the end of the text that had already streamed, which is where it
 * belongs in reading order.
 */
function resolveInsertion(content: string, doc: Document, rawOffset: number): Insertion {
    const offset = Math.min(Math.max(rawOffset, 0), content.length);
    if (doc.blocks.length === 0) {
        return spacedInsertion(content, content.length);
    }

    const region = enclosingRegion(doc, offset);
    if (region?.unclosed) {
        const before = anchorAtOrBefore(doc, region.blockStart);
        if (before !== null) {
            return spacedInsertion(content, before);
        }
        const after = anchorAtOrAfter(doc, region.blockStart);
        return after !== null ? spacedInsertion(content, after) : ownParagraph(region.blockStart);
    }
    if (region) {
        const after = anchorAtOrAfter(doc, region.end);
        if (after !== null) {
            return spacedInsertion(content, after);
        }
        const before = anchorAtOrBefore(doc, region.start);
        return before !== null ? spacedInsertion(content, before) : ownParagraph(region.blockEnd);
    }

    const before = anchorAtOrBefore(doc, offset);
    if (before !== null) {
        return spacedInsertion(content, before);
    }

    // Nothing before the offset can host a chip. Text later in the same block
    // still can — an offset on a list marker belongs with that item's text —
    // but jumping into a later block would reorder the chip against content
    // that streamed after it.
    const block = blockAt(doc, offset);
    const after = anchorAtOrAfter(doc, offset);
    if (after !== null && block && after <= block.end) {
        return spacedInsertion(content, after);
    }
    return ownParagraph(block ? block.start : offset);
}

/** Convert a character-counted offset into a UTF-16 index into `content`. */
function toolCallOffset(toolCall: ToolCall, characters: string[]): number {
    const offset = toolCall.content_offset;
    if (offset === undefined || !Number.isFinite(offset)) {
        return 0;
    }
    const clamped = Math.min(Math.max(Math.trunc(offset), 0), characters.length);
    return characters.slice(0, clamped).join("").length;
}

/** Splice a chip link for every tool call into `content`. */
export function addInlineToolCalls(content: string, toolCalls: ToolCall[]): string {
    if (toolCalls.length === 0) {
        return content;
    }

    const doc = parseDocument(content);
    const characters = [...content];
    const insertions = toolCalls
        .map((toolCall, index) => ({
            index,
            insertion: resolveInsertion(content, doc, toolCallOffset(toolCall, characters)),
            toolCall,
        }))
        .sort((left, right) => left.insertion.at - right.insertion.at || left.index - right.index);

    let cursor = 0;
    let markdown = "";
    for (const { index, insertion, toolCall } of insertions) {
        const at = Math.max(insertion.at, cursor);
        markdown = `${markdown}${content.slice(cursor, at)}`;
        let prefix = insertion.prefix;
        if (markdown === "") {
            prefix = "";
        } else if (prefix === "" && !/\s$/.test(markdown)) {
            // Only reachable when the previous chip ended here; keep them apart.
            prefix = " ";
        }
        markdown = `${markdown}${prefix}${toolCallLink(toolCall, index)}${insertion.suffix}`;
        cursor = at;
    }

    return `${markdown}${content.slice(cursor)}`;
}
