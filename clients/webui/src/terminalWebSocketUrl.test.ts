import { describe, expect, it } from "vitest";
import { terminalWebSocketUrl } from "./api";

describe("terminalWebSocketUrl", () => {
    it("uses a same-origin WebSocket URL and encodes the session ID", () => {
        expect(
            terminalWebSocketUrl(
                "session/with spaces",
                132,
                41,
                "https://agentspace.example/chat",
            ),
        ).toBe(
            "wss://agentspace.example/api/sessions/session%2Fwith%20spaces/terminal?cols=132&rows=41",
        );
    });

    it("uses ws for an http deployment", () => {
        expect(
            terminalWebSocketUrl("session-1", 80, 24, "http://127.0.0.1:8003/"),
        ).toBe(
            "ws://127.0.0.1:8003/api/sessions/session-1/terminal?cols=80&rows=24",
        );
    });
});
