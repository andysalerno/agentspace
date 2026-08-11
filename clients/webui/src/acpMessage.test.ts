import { describe, expect, it } from "vitest";
import { applyAcpUpdateToAssistant } from "./acpMessage";
import type { AcpSessionUpdate, ChatMessage } from "./types";

function assistant(): ChatMessage {
    return {
        message_id: "msg-1",
        session_id: "session-1",
        role: "assistant",
        content: "",
        created_at: "2026-01-01T00:00:00Z",
    };
}

function applyAll(updates: AcpSessionUpdate[]): ChatMessage {
    return updates.reduce(applyAcpUpdateToAssistant, assistant());
}

describe("applyAcpUpdateToAssistant", () => {
    it("streams terminal output without leaking the terminal handle", () => {
        // How pi (through the pi-acp adapter) reports a shell tool: the content
        // block is an opaque handle and the output arrives as `_meta` deltas.
        const message = applyAll([
            {
                sessionUpdate: "tool_call",
                toolCallId: "call-1",
                title: "bash",
                status: "in_progress",
                content: [{ type: "terminal", terminalId: "term-1" }],
            },
            {
                sessionUpdate: "tool_call_update",
                toolCallId: "call-1",
                content: [{ type: "terminal", terminalId: "term-1" }],
                _meta: { terminal_output: { data: "hello " } },
            },
            {
                sessionUpdate: "tool_call_update",
                toolCallId: "call-1",
                status: "completed",
                content: [{ type: "terminal", terminalId: "term-1" }],
                _meta: { terminal_output: { data: "world" } },
            },
        ]);

        const toolCall = message.tool_calls?.[0];
        expect(toolCall?.tool).toBe("bash");
        expect(toolCall?.status).toBe("completed");
        expect(toolCall?.output).toBe("hello world");
        expect(toolCall?.output).not.toContain("terminalId");
    });

    it("keeps text tool output that arrives in content blocks", () => {
        const message = applyAll([
            { sessionUpdate: "tool_call", toolCallId: "call-1", title: "read" },
            {
                sessionUpdate: "tool_call_update",
                toolCallId: "call-1",
                content: [{ type: "content", content: { type: "text", text: "file body" } }],
            },
        ]);

        expect(message.tool_calls?.[0].output).toBe("file body");
    });

    it("appends message chunks and ignores terminal blocks in them", () => {
        const message = applyAll([
            {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: "part one " },
            },
            {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: "part two" },
            },
        ]);

        expect(message.content).toBe("part one part two");
    });
});
