/*
 * Inline tool-call chips for assistant markdown.
 *
 * Tool calls are recorded with a `content_offset` — the number of characters of
 * assistant text that had streamed when the call started. To draw a chip at
 * that point the offset is turned into a markdown link that the renderer swaps
 * for a button. Splicing text into markdown is only safe at some positions, so
 * every offset is first moved to the nearest position that cannot corrupt the
 * surrounding document (never inside a fence, a code span, a link, or ahead of
 * a block marker), and labels are collapsed, truncated and escaped so a long or
 * multi-line tool title can neither blow up the chip nor break out of it.
 */
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
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
const fenceOpen = /^ {0,3}(`{3,}|~{3,})/;
const heading = /^ {0,3}#{1,6}(?:\s|$)/;
const blockQuote = /^ {0,3}>/;
const listItem = /^ {0,3}(?:[-*+]|\d{1,9}[.)])(?:\s|$)/;
const thematicBreak = /^ {0,3}(?:(?:\*\s*){3,}|(?:-\s*){3,}|(?:_\s*){3,})$/;
const htmlBlock = /^ {0,3}<[a-zA-Z!/?]/;
const indentedCode = /^(?: {4}|\t)/;
const tableDelimiter = /^[|\-: \t]+$/;

/**
 * How a line may be joined with a chip.
 *
 * - `text`: plain paragraph text; safe anywhere on the line.
 * - `structure`: heading, list item or block quote; safe except at line start,
 *   where a chip would displace the marker.
 * - `fence` / `code` / `table` / `raw`: never safe; chips move to a block
 *   boundary instead.
 */
type LineKind = "blank" | "code" | "fence" | "raw" | "structure" | "table" | "text";

type Line = {
    /** Position of this line within the scanned content. */
    index: number;
    /** Index of the first character of the line. */
    start: number;
    /** Index just past the last character of the line, excluding the newline. */
    end: number;
    /** Index of the next line's start, or the content length for the last line. */
    next: number;
    kind: LineKind;
    /** First line of a fenced code block or a table, which a chip may precede. */
    blockStart: boolean;
    /** Set on the opening line of a fence that never closes (still streaming). */
    unclosed: boolean;
};

type Region = { start: number; end: number };

type Insertion = { at: number; prefix: string; suffix: string };

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

function scanLines(content: string): Line[] {
    const lines: Line[] = [];
    let cursor = 0;
    for (;;) {
        const newline = content.indexOf("\n", cursor);
        const end = newline < 0 ? content.length : newline;
        lines.push({
            index: lines.length,
            start: cursor,
            end,
            next: newline < 0 ? content.length : newline + 1,
            kind: "blank",
            blockStart: false,
            unclosed: false,
        });
        if (newline < 0) {
            break;
        }
        cursor = newline + 1;
    }
    return lines;
}

function closesFence(text: string, marker: string): boolean {
    const trimmed = text.trim();
    return trimmed.startsWith(marker) && trimmed.split("").every((char) => char === marker[0]);
}

function classifyLine(text: string): LineKind {
    if (text.trim() === "") {
        return "blank";
    }
    if (indentedCode.test(text) || thematicBreak.test(text) || htmlBlock.test(text)) {
        return "raw";
    }
    if (heading.test(text) || blockQuote.test(text) || listItem.test(text)) {
        return "structure";
    }
    return "text";
}

function isTableDelimiter(text: string): boolean {
    const trimmed = text.trim();
    return trimmed.includes("|") && trimmed.includes("-") && tableDelimiter.test(trimmed);
}

function markFences(content: string, lines: Line[]): void {
    let marker: string | null = null;
    let opened: Line | null = null;
    for (const line of lines) {
        const text = content.slice(line.start, line.end);
        if (marker === null) {
            const opening = fenceOpen.exec(text);
            if (opening) {
                line.kind = "fence";
                line.blockStart = true;
                marker = opening[1];
                opened = line;
                continue;
            }
            line.kind = classifyLine(text);
            continue;
        }
        line.kind = closesFence(text, marker) ? "fence" : "code";
        if (line.kind === "fence") {
            marker = null;
            opened = null;
        }
    }
    if (opened) {
        // A fence that is still streaming swallows everything to the end, so a
        // chip inside it has to be placed before the block instead.
        opened.unclosed = true;
    }
}

function markTables(content: string, lines: Line[]): void {
    for (const [index, line] of lines.entries()) {
        if (line.kind === "code" || line.kind === "fence") {
            continue;
        }
        const text = content.slice(line.start, line.end);
        if (!isTableDelimiter(text)) {
            continue;
        }
        const header = lines[index - 1];
        if (!header || header.kind === "blank" || !content.slice(header.start, header.end).includes("|")) {
            continue;
        }
        let first = line;
        for (let cursor = index - 1; cursor >= 0; cursor -= 1) {
            const row = lines[cursor];
            if (row.kind === "blank" || row.kind === "code" || row.kind === "fence") break;
            if (!content.slice(row.start, row.end).includes("|")) break;
            row.kind = "table";
            first = row;
        }
        first.blockStart = true;
        for (let cursor = index; cursor < lines.length; cursor += 1) {
            const row = lines[cursor];
            if (row.kind === "blank" || row.kind === "code" || row.kind === "fence") break;
            if (cursor > index && !content.slice(row.start, row.end).includes("|")) break;
            row.kind = "table";
        }
    }
}

/** Code spans and links on a single line: splicing inside either breaks them. */
function inlineRegions(content: string, line: Line): Region[] {
    const text = content.slice(line.start, line.end);
    const regions: Region[] = [];

    const backticks = /`+/g;
    let open: RegExpExecArray | null = null;
    for (let run = backticks.exec(text); run !== null; run = backticks.exec(text)) {
        if (open === null) {
            open = run;
            continue;
        }
        if (run[0].length === open[0].length) {
            regions.push({
                start: line.start + open.index,
                end: line.start + run.index + run[0].length,
            });
            open = null;
        }
    }

    const links = /!?\[[^\]\n]*\]\([^)\n]*\)/g;
    for (let link = links.exec(text); link !== null; link = links.exec(text)) {
        regions.push({
            start: line.start + link.index,
            end: line.start + link.index + link[0].length,
        });
    }

    return regions;
}

function lineAt(lines: Line[], offset: number): Line {
    for (const line of lines) {
        if (offset <= line.end) {
            return line;
        }
    }
    return lines[lines.length - 1];
}

function blockStartLine(lines: Line[], line: Line): Line {
    let current = line;
    while (!current.blockStart && current.index > 0) {
        current = lines[current.index - 1];
    }
    return current;
}

function blockEnd(lines: Line[], line: Line, kinds: LineKind[]): number {
    let index = line.index;
    while (index + 1 < lines.length && kinds.includes(lines[index + 1].kind)) {
        index += 1;
    }
    return lines[index].next;
}

function previousContentLine(lines: Line[], line: Line): Line | null {
    for (let cursor = line.index - 1; cursor >= 0; cursor -= 1) {
        if (lines[cursor].kind !== "blank") {
            return lines[cursor];
        }
    }
    return null;
}

function trimmedLineEnd(content: string, line: Line): number {
    let end = line.end;
    while (end > line.start && /\s/.test(content.charAt(end - 1))) {
        end -= 1;
    }
    return end;
}

function spacedInsertion(content: string, at: number): Insertion {
    const before = content.charAt(at - 1);
    const after = content.charAt(at);
    return {
        at,
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
 */
function resolveInsertion(content: string, lines: Line[], rawOffset: number): Insertion {
    let offset = Math.min(Math.max(rawOffset, 0), content.length);
    let line = lineAt(lines, offset);
    const opensBlock = line.blockStart && content.slice(line.start, offset).trim() === "";

    if ((line.kind === "fence" || line.kind === "code") && !opensBlock) {
        const opening = blockStartLine(lines, line);
        if (opening.unclosed) {
            offset = opening.start;
            line = opening;
        } else {
            offset = blockEnd(lines, line, ["fence", "code"]);
            line = lineAt(lines, offset);
        }
    } else if (line.kind === "table" && !opensBlock) {
        offset = blockEnd(lines, line, ["table"]);
        line = lineAt(lines, offset);
    } else if (line.kind !== "fence" && line.kind !== "code" && line.kind !== "table") {
        const region = inlineRegions(content, line).find(
            (candidate) => candidate.start < offset && offset < candidate.end,
        );
        if (region) {
            offset = region.end;
        }
    }

    if (content.slice(line.start, offset).trim() !== "") {
        if (line.kind === "text" || line.kind === "structure") {
            return spacedInsertion(content, offset);
        }
        return ownParagraph(line.next);
    }

    if (content.slice(offset).trim() === "") {
        return spacedInsertion(content, offset);
    }

    // Blank lines cannot host a chip; the next line that carries content can.
    while (line.kind === "blank" && line.index + 1 < lines.length) {
        line = lines[line.index + 1];
        offset = line.start;
    }

    const previous = previousContentLine(lines, line);
    if (previous && (previous.kind === "text" || previous.kind === "structure")) {
        return spacedInsertion(content, trimmedLineEnd(content, previous));
    }
    if (line.kind === "text") {
        return spacedInsertion(content, line.start);
    }
    return ownParagraph(line.start);
}

function toolCallOffset(toolCall: ToolCall, content: string): number {
    const offset = toolCall.content_offset;
    if (offset === undefined || !Number.isFinite(offset)) {
        return 0;
    }
    const characters = [...content];
    const clamped = Math.min(Math.max(Math.trunc(offset), 0), characters.length);
    return characters.slice(0, clamped).join("").length;
}

/** Splice a chip link for every tool call into `content`. */
export function addInlineToolCalls(content: string, toolCalls: ToolCall[]): string {
    if (toolCalls.length === 0) {
        return content;
    }

    const lines = scanLines(content);
    markFences(content, lines);
    markTables(content, lines);

    const insertions = toolCalls
        .map((toolCall, index) => ({
            index,
            insertion: resolveInsertion(content, lines, toolCallOffset(toolCall, content)),
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
