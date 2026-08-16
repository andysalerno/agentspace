import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Terminal from "./Terminal";
import type { TerminalStatus } from "./types";

const xtermMocks = vi.hoisted(() => {
    class Disposable {
        disposed = false;

        dispose() {
            this.disposed = true;
        }
    }

    class FakeTerminal {
        static instances: FakeTerminal[] = [];

        readonly dataDisposable = new Disposable();
        readonly binaryDisposable = new Disposable();
        readonly writes: Array<string | Uint8Array> = [];
        readonly addons: Array<{ dispose?: () => void; activate?: (terminal: FakeTerminal) => void }> =
            [];
        cols = 80;
        rows = 24;
        disposed = false;
        focusCalls = 0;
        scrollToTopCalls = 0;
        scrollToBottomCalls = 0;
        options: Record<string, unknown>;
        unicode = { activeVersion: "" };
        dataHandler: ((data: string) => void) | null = null;
        binaryHandler: ((data: string) => void) | null = null;
        keyHandler: ((event: KeyboardEvent) => boolean) | null = null;

        constructor(options: Record<string, unknown>) {
            this.options = { ...options };
            FakeTerminal.instances.push(this);
        }

        loadAddon(addon: {
            dispose?: () => void;
            activate?: (terminal: FakeTerminal) => void;
        }) {
            this.addons.push(addon);
            addon.activate?.(this);
        }

        open() {}

        attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean) {
            this.keyHandler = handler;
        }

        onData(handler: (data: string) => void) {
            this.dataHandler = handler;
            return this.dataDisposable;
        }

        onBinary(handler: (data: string) => void) {
            this.binaryHandler = handler;
            return this.binaryDisposable;
        }

        write(data: string | Uint8Array) {
            this.writes.push(data);
        }

        focus() {
            this.focusCalls += 1;
        }

        scrollToTop() {
            this.scrollToTopCalls += 1;
        }

        scrollToBottom() {
            this.scrollToBottomCalls += 1;
        }

        scrollPages() {}

        scrollLines() {}

        dispose() {
            this.disposed = true;
            for (const addon of this.addons) {
                addon.dispose?.();
            }
        }

        emitData(data: string) {
            this.dataHandler?.(data);
        }

        emitBinary(data: string) {
            this.binaryHandler?.(data);
        }
    }

    class FakeFitAddon {
        static instances: FakeFitAddon[] = [];

        terminal: FakeTerminal | null = null;
        disposed = false;
        nextDimensions: { cols: number; rows: number } | null = null;

        constructor() {
            FakeFitAddon.instances.push(this);
        }

        activate(terminal: FakeTerminal) {
            this.terminal = terminal;
        }

        fit() {
            if (this.terminal !== null && this.nextDimensions !== null) {
                this.terminal.cols = this.nextDimensions.cols;
                this.terminal.rows = this.nextDimensions.rows;
            }
        }

        dispose() {
            this.disposed = true;
        }
    }

    class FakeUnicodeAddon {
        static instances: FakeUnicodeAddon[] = [];

        disposed = false;

        constructor() {
            FakeUnicodeAddon.instances.push(this);
        }

        activate() {}

        dispose() {
            this.disposed = true;
        }
    }

    class FakeWebglAddon {
        static instances: FakeWebglAddon[] = [];
        static throwOnCreate = false;

        disposed = false;
        contextLossHandler: (() => void) | null = null;
        contextLossDisposable = new Disposable();

        constructor() {
            if (FakeWebglAddon.throwOnCreate) {
                throw new Error("no webgl");
            }
            FakeWebglAddon.instances.push(this);
        }

        activate() {}

        onContextLoss(handler: () => void) {
            this.contextLossHandler = handler;
            return this.contextLossDisposable;
        }

        loseContext() {
            this.contextLossHandler?.();
        }

        dispose() {
            this.disposed = true;
        }
    }

    return {
        FakeFitAddon,
        FakeTerminal,
        FakeUnicodeAddon,
        FakeWebglAddon,
    };
});

vi.mock("@xterm/xterm", () => ({ Terminal: xtermMocks.FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: xtermMocks.FakeFitAddon }));
vi.mock("@xterm/addon-unicode11", () => ({
    Unicode11Addon: xtermMocks.FakeUnicodeAddon,
}));
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: xtermMocks.FakeWebglAddon }));

class FakeResizeObserver {
    static instances: FakeResizeObserver[] = [];

    callback: ResizeObserverCallback;
    disconnected = false;
    observed: Element[] = [];

    constructor(callback: ResizeObserverCallback) {
        this.callback = callback;
        FakeResizeObserver.instances.push(this);
    }

    observe(element: Element) {
        this.observed.push(element);
    }

    unobserve() {}

    disconnect() {
        this.disconnected = true;
    }

    trigger() {
        this.callback([], this);
    }
}

class FakeWebSocket {
    static readonly CONNECTING = 0;
    static readonly OPEN = 1;
    static readonly CLOSING = 2;
    static readonly CLOSED = 3;
    static instances: FakeWebSocket[] = [];

    readonly url: string;
    binaryType = "blob";
    readyState = FakeWebSocket.CONNECTING;
    sent: Array<string | ArrayBuffer> = [];
    closeCalls: Array<{ code?: number; reason?: string }> = [];
    onopen: (() => void) | null = null;
    onmessage: ((event: MessageEvent) => void) | null = null;
    onerror: (() => void) | null = null;
    onclose: ((event: CloseEvent) => void) | null = null;

    constructor(url: string | URL) {
        this.url = String(url);
        FakeWebSocket.instances.push(this);
    }

    send(data: string | ArrayBuffer) {
        this.sent.push(data);
    }

    close(code?: number, reason?: string) {
        this.closeCalls.push({ code, reason });
        this.readyState = FakeWebSocket.CLOSED;
        this.onclose?.({ code: code ?? 1000, reason: reason ?? "" } as CloseEvent);
    }

    open() {
        this.readyState = FakeWebSocket.OPEN;
        this.onopen?.();
    }

    message(data: string | ArrayBuffer) {
        this.onmessage?.({ data } as MessageEvent);
    }

    serverClose(code: number, reason = "") {
        this.readyState = FakeWebSocket.CLOSED;
        this.onclose?.({ code, reason } as CloseEvent);
    }
}

const STATUS: TerminalStatus = {
    state: "running",
    exit_status: null,
    attach_kind: "attached",
    attachment_count: 1,
};

function readyFrame() {
    return JSON.stringify({
        type: "ready",
        attachment_id: "attachment-one",
        cols: 80,
        rows: 24,
        terminal: STATUS,
    });
}

function props() {
    return {
        sessionId: "session-one",
        darkMode: false,
        reconnectKey: 0,
        onAttachmentChange: vi.fn(),
        onConnectionStateChange: vi.fn(),
        onLifecycleStatus: vi.fn(),
    };
}

beforeEach(() => {
    xtermMocks.FakeTerminal.instances = [];
    xtermMocks.FakeFitAddon.instances = [];
    xtermMocks.FakeUnicodeAddon.instances = [];
    xtermMocks.FakeWebglAddon.instances = [];
    xtermMocks.FakeWebglAddon.throwOnCreate = false;
    FakeResizeObserver.instances = [];
    FakeWebSocket.instances = [];
    vi.stubGlobal("ResizeObserver", FakeResizeObserver);
    vi.stubGlobal("WebSocket", FakeWebSocket);
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
    vi.unstubAllGlobals();
});

describe("Terminal", () => {
    it("keeps scrollback browser-local and blocks terminal input until exit", () => {
        const onScrollbackModeChange = vi.fn();
        const base = props();
        const { rerender } = render(
            <Terminal
                {...base}
                onScrollbackModeChange={onScrollbackModeChange}
                scrollbackMode={false}
            />,
        );
        const terminal = xtermMocks.FakeTerminal.instances[0];

        rerender(
            <Terminal
                {...base}
                onScrollbackModeChange={onScrollbackModeChange}
                scrollbackMode
            />,
        );
        expect(terminal.scrollToTopCalls).toBe(1);
        expect(
            terminal.keyHandler?.(
                new KeyboardEvent("keydown", { key: "q" }),
            ),
        ).toBe(false);
        expect(onScrollbackModeChange).toHaveBeenCalledWith(false);

        rerender(
            <Terminal
                {...base}
                onScrollbackModeChange={onScrollbackModeChange}
                scrollbackMode={false}
            />,
        );
        expect(terminal.scrollToBottomCalls).toBeGreaterThan(0);
    });

    it("uses the current-origin API URL and preserves binary IO", () => {
        const callbacks = props();
        render(<Terminal {...callbacks} />);
        const socket = FakeWebSocket.instances[0];
        const terminal = xtermMocks.FakeTerminal.instances[0];

        expect(socket.url).toBe(
            "ws://localhost:3000/api/sessions/session-one/terminal/ws",
        );
        expect(socket.binaryType).toBe("arraybuffer");
        expect(terminal.options.scrollback).toBe(100_000);
        expect(terminal.unicode.activeVersion).toBe("11");

        act(() => {
            socket.open();
            socket.message(readyFrame());
            terminal.emitData("é");
            terminal.emitBinary("\x00\xff");
            socket.message(Uint8Array.from([0, 0xff, 65, 0x80]).buffer);
        });

        const binaryFrames = socket.sent.filter(
            (frame): frame is ArrayBuffer => frame instanceof ArrayBuffer,
        );
        expect(Array.from(new Uint8Array(binaryFrames[0]))).toEqual([0xc3, 0xa9]);
        expect(Array.from(new Uint8Array(binaryFrames[1]))).toEqual([0, 0xff]);
        expect(Array.from(terminal.writes[0] as Uint8Array)).toEqual([0, 0xff, 65, 0x80]);
        expect(callbacks.onAttachmentChange).toHaveBeenLastCalledWith(
            expect.objectContaining({ attachmentId: "attachment-one" }),
        );
        expect(callbacks.onLifecycleStatus).toHaveBeenCalledWith(STATUS);
        expect(terminal.focusCalls).toBe(1);
    });

    it("sends dimensions after ready and only after actual size changes", () => {
        render(<Terminal {...props()} />);
        const socket = FakeWebSocket.instances[0];
        const fit = xtermMocks.FakeFitAddon.instances[0];
        const observer = FakeResizeObserver.instances[0];

        act(() => {
            socket.open();
            socket.message(readyFrame());
        });
        expect(socket.sent).toContain(JSON.stringify({
            type: "resize",
            cols: 80,
            rows: 24,
        }));

        fit.nextDimensions = { cols: 100, rows: 30 };
        act(() => observer.trigger());
        act(() => observer.trigger());
        const resizeFrames = socket.sent.filter((frame) => typeof frame === "string");
        expect(resizeFrames).toEqual([
            JSON.stringify({ type: "resize", cols: 80, rows: 24 }),
            JSON.stringify({ type: "resize", cols: 100, rows: 30 }),
        ]);
    });

    it.each([1006, 1011, 4429, 4503])(
        "reconnects retryable close code %s",
        (code) => {
            vi.useFakeTimers();
            render(<Terminal {...props()} />);
            FakeWebSocket.instances[0].serverClose(code, "retryable");
            expect(FakeWebSocket.instances).toHaveLength(1);
            void act(() => vi.advanceTimersByTime(250));
            expect(FakeWebSocket.instances).toHaveLength(2);
        },
    );

    it.each([1000, 4404, 4409])(
        "does not reconnect terminal close code %s",
        (code) => {
            vi.useFakeTimers();
            render(<Terminal {...props()} />);
            FakeWebSocket.instances[0].serverClose(code, "not retryable");
            void act(() => vi.runAllTimers());
            expect(FakeWebSocket.instances).toHaveLength(1);
        },
    );

    it("bounds exponential reconnect attempts", () => {
        vi.useFakeTimers();
        render(<Terminal {...props()} />);
        for (const delay of [250, 500, 1_000, 2_000, 4_000]) {
            FakeWebSocket.instances.at(-1)?.serverClose(4503, "unavailable");
            void act(() => vi.advanceTimersByTime(delay));
        }
        expect(FakeWebSocket.instances).toHaveLength(6);
        FakeWebSocket.instances.at(-1)?.serverClose(4503, "still unavailable");
        void act(() => vi.runAllTimers());
        expect(FakeWebSocket.instances).toHaveLength(6);
    });

    it("does not reconnect normal detach or terminal exit", () => {
        vi.useFakeTimers();
        render(<Terminal {...props()} />);
        const socket = FakeWebSocket.instances[0];
        act(() => {
            socket.open();
            socket.message(readyFrame());
            socket.message(JSON.stringify({
                type: "exited",
                state: "exited",
                exit_status: 7,
                terminal: { ...STATUS, state: "exited", exit_status: 7 },
            }));
            socket.serverClose(1006);
            vi.runAllTimers();
        });
        expect(FakeWebSocket.instances).toHaveLength(1);
    });

    it("updates theme, falls back on WebGL loss, and fully disposes resources", () => {
        const callbacks = props();
        const rendered = render(<Terminal {...callbacks} />);
        const terminal = xtermMocks.FakeTerminal.instances[0];
        const webgl = xtermMocks.FakeWebglAddon.instances[0];
        const socket = FakeWebSocket.instances[0];
        const observer = FakeResizeObserver.instances[0];

        rendered.rerender(<Terminal {...callbacks} darkMode />);
        expect((terminal.options.theme as { background: string }).background).toBe("#141414");

        act(() => webgl.loseContext());
        expect(webgl.disposed).toBe(true);
        expect(socket.closeCalls).toHaveLength(0);

        rendered.rerender(<Terminal {...callbacks} darkMode sessionId="session-two" />);
        expect(terminal.disposed).toBe(true);
        expect(terminal.dataDisposable.disposed).toBe(true);
        expect(terminal.binaryDisposable.disposed).toBe(true);
        expect(observer.disconnected).toBe(true);
        expect(socket.closeCalls).toEqual([{ code: 1000, reason: "view detached" }]);
        expect(FakeWebSocket.instances).toHaveLength(2);

        rendered.unmount();
        expect(xtermMocks.FakeTerminal.instances[1].disposed).toBe(true);
    });

    it("falls back when WebGL cannot be created without detaching", () => {
        vi.useFakeTimers();
        xtermMocks.FakeWebglAddon.throwOnCreate = true;
        const rendered = render(<Terminal {...props()} />);
        void act(() => vi.runOnlyPendingTimers());
        expect(rendered.getByText(/WebGL is unavailable/)).toBeTruthy();
        expect(FakeWebSocket.instances[0].closeCalls).toHaveLength(0);
    });

    it("does not steal focus from an open dialog after attach", () => {
        const dialog = document.createElement("div");
        dialog.setAttribute("role", "dialog");
        const input = document.createElement("input");
        dialog.append(input);
        document.body.append(dialog);
        input.focus();

        render(<Terminal {...props()} />);
        const socket = FakeWebSocket.instances[0];
        act(() => {
            socket.open();
            socket.message(readyFrame());
        });
        expect(xtermMocks.FakeTerminal.instances[0].focusCalls).toBe(0);
        dialog.remove();
    });
});
