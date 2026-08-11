import type { AcpSessionUpdate, ChatMessage } from "./types";
import { characterLength } from "./toolCallMarkdown";

/**
 * Folds an ACP `session/update` into the assistant message being streamed.
 *
 * ACP agents differ in how much they route through the protocol: opencode
 * delegates shell work back over ACP, while pi (via the `pi-acp` adapter) runs
 * tools in-process and reports their output as `_meta` deltas on the tool call.
 * Both shapes have to land in the same `ChatMessage`.
 */
export function applyAcpUpdateToAssistant(
    message: ChatMessage,
    update: AcpSessionUpdate,
): ChatMessage {
    if (update.sessionUpdate === "agent_message_chunk") {
        return { ...message, content: `${message.content}${contentText(update.content)}` };
    }

    if (update.sessionUpdate === "agent_thought_chunk") {
        return {
            ...message,
            reasoning: `${message.reasoning ?? ""}${contentText(update.content)}`,
        };
    }

    if (update.sessionUpdate === "plan") {
        return {
            ...message,
            reasoning: `${message.reasoning ?? ""}${JSON.stringify({ plan: update.entries }, null, 2)}`,
        };
    }

    if (update.sessionUpdate === "tool_call" || update.sessionUpdate === "tool_call_update") {
        return upsertToolCall(message, update);
    }

    return message;
}

function upsertToolCall(message: ChatMessage, update: AcpSessionUpdate): ChatMessage {
    const toolCallId = typeof update.toolCallId === "string" ? update.toolCallId : undefined;
    const toolCalls = [...(message.tool_calls ?? [])];
    let toolIndex = toolCallId
        ? toolCalls.findIndex((toolCall) => toolCall.tool_call_id === toolCallId)
        : -1;

    if (toolIndex < 0) {
        toolCalls.push({
            tool: typeof update.title === "string" && update.title ? update.title : toolCallId ?? "tool",
            tool_call_id: toolCallId,
            content_offset: characterLength(message.content.trim()),
        });
        toolIndex = toolCalls.length - 1;
    }

    const current = toolCalls[toolIndex];
    const terminalChunk = terminalOutputDelta(update);
    const output = toolOutput(update) ?? current.output;
    toolCalls[toolIndex] = {
        ...current,
        tool: typeof update.title === "string" && update.title ? update.title : current.tool,
        status: typeof update.status === "string" ? update.status : current.status,
        kind: typeof update.kind === "string" ? update.kind : current.kind,
        input: Object.hasOwn(update, "rawInput") ? jsonText(update.rawInput) : current.input,
        output: terminalChunk === undefined ? output : `${output ?? ""}${terminalChunk}`,
    };
    return { ...message, tool_calls: toolCalls };
}

/**
 * Incremental terminal output from ACP agents that stream shell tool output
 * through `_meta` (for example the `pi-acp` adapter) instead of tool content.
 */
function terminalOutputDelta(update: AcpSessionUpdate): string | undefined {
    const meta = update._meta;
    if (typeof meta !== "object" || meta === null) {
        return undefined;
    }
    const terminalOutput = (meta as Record<string, unknown>).terminal_output;
    if (typeof terminalOutput !== "object" || terminalOutput === null) {
        return undefined;
    }
    const data = (terminalOutput as Record<string, unknown>).data;
    return typeof data === "string" && data !== "" ? data : undefined;
}

function toolOutput(update: AcpSessionUpdate): string | undefined {
    if (Object.hasOwn(update, "rawOutput")) {
        return jsonText(update.rawOutput);
    }
    const text = contentText(update.content);
    return text || undefined;
}

function jsonText(value: unknown): string | undefined {
    if (value == null) {
        return undefined;
    }
    return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

function contentText(content: unknown): string {
    if (Array.isArray(content)) {
        return content.map(contentText).join("");
    }
    if (content == null) {
        return "";
    }
    if (
        typeof content === "string" ||
        typeof content === "number" ||
        typeof content === "boolean" ||
        typeof content === "bigint"
    ) {
        return String(content);
    }
    if (typeof content === "symbol") {
        return content.description ?? "";
    }
    if (typeof content !== "object") {
        return "";
    }
    const block = content as Record<string, unknown>;
    if (block.type === "text") {
        return typeof block.text === "string" ? block.text : "";
    }
    if (block.type === "content") {
        return contentText(block.content);
    }
    // Terminal blocks are an opaque handle; the output itself streams in
    // through `_meta.terminal_output` and is appended by `upsertToolCall`.
    if (block.type === "terminal") {
        return "";
    }
    return JSON.stringify(block);
}
