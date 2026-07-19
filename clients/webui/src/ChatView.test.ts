import { describe, expect, it } from "vitest";

import { applyEventToAssistant, sessionRestartMessages } from "./ChatView";
import type { ChatMessage } from "./types";

describe("fresh-session stream handoff", () => {
  it("discards transient old-session rendering and accepts fresh events", () => {
    const message: ChatMessage = {
      message_id: "assistant-one",
      session_id: "session-one",
      role: "assistant",
      content: "old answer",
      reasoning: "old reasoning",
      created_at: "2026-01-01T00:00:00Z",
      tool_calls: [{ tool: "old-tool", output: "old output" }],
    };

    const restarted = applyEventToAssistant(message, {
      type: "agentspace/session-restarted",
      ts: "2026-01-01T00:00:01Z",
      restart_count: 1,
    });
    expect(restarted).toMatchObject({
      content: "",
      reasoning: "",
      tool_calls: [],
    });

    const fresh = applyEventToAssistant(restarted, {
      type: "text_delta",
      ts: "2026-01-01T00:00:02Z",
      content: "fresh answer",
    });
    expect(fresh.content).toBe("fresh answer");
  });

  it("provides replacement transcript records for cache retargeting", () => {
    const userMessage: ChatMessage = {
      message_id: "fresh-user",
      session_id: "session-one",
      role: "user",
      content: "new topic",
      created_at: "2026-01-01T00:00:01Z",
      tool_calls: [],
    };
    const assistantMessage: ChatMessage = {
      message_id: "fresh-assistant",
      session_id: "session-one",
      role: "assistant",
      content: "",
      created_at: "2026-01-01T00:00:01Z",
      tool_calls: [],
    };

    expect(sessionRestartMessages({
      type: "agentspace/session-restarted",
      ts: "2026-01-01T00:00:01Z",
      restart_count: 1,
      user_message: userMessage,
      assistant_message: assistantMessage,
    })).toEqual({ userMessage, assistantMessage });
  });
});
