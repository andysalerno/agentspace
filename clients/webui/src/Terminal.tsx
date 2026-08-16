import { useEffect, useRef, useState } from "react";
import { Terminal as XtermTerminal } from "@xterm/xterm";
import type { IDisposable, ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import type {
    TerminalReadyFrame,
    TerminalServerFrame,
    TerminalStatus,
} from "./types";
import "@xterm/xterm/css/xterm.css";
import "./terminal.css";

const MAX_RECONNECT_ATTEMPTS = 5;
const RECONNECT_BASE_MS = 250;
const RECONNECT_MAX_MS = 4_000;
const RETRYABLE_CLOSE_CODES = new Set([1006, 1011, 4429, 4503]);

export type TerminalConnectionState =
    | "connecting"
    | "ready"
    | "reconnecting"
    | "disconnected"
    | "exited"
    | "error";

export type TerminalAttachment = {
    attachmentId: string;
    cols: number;
    rows: number;
    terminal: TerminalStatus;
};

type TerminalProps = {
    sessionId: string;
    darkMode: boolean;
    reconnectKey: number;
    scrollbackMode?: boolean;
    onAttachmentChange: (attachment: TerminalAttachment | null) => void;
    onConnectionStateChange: (state: TerminalConnectionState) => void;
    onLifecycleStatus: (status: TerminalStatus) => void;
    onScrollbackModeChange?: (active: boolean) => void;
};

function terminalTheme(darkMode: boolean): ITheme {
    if (darkMode) {
        return {
            background: "#141414",
            foreground: "#d4d4d4",
            cursor: "#ffffff",
            cursorAccent: "#141414",
            selectionBackground: "#264f78",
            black: "#141414",
            red: "#f14c4c",
            green: "#23d18b",
            yellow: "#f5f543",
            blue: "#3b8eea",
            magenta: "#d670d6",
            cyan: "#29b8db",
            white: "#e5e5e5",
            brightBlack: "#666666",
            brightRed: "#f14c4c",
            brightGreen: "#23d18b",
            brightYellow: "#f5f543",
            brightBlue: "#3b8eea",
            brightMagenta: "#d670d6",
            brightCyan: "#29b8db",
            brightWhite: "#ffffff",
        };
    }
    return {
        background: "#ffffff",
        foreground: "#242424",
        cursor: "#242424",
        cursorAccent: "#ffffff",
        selectionBackground: "#add6ff",
        black: "#242424",
        red: "#cd3131",
        green: "#107c10",
        yellow: "#795e26",
        blue: "#0451a5",
        magenta: "#bc05bc",
        cyan: "#0598bc",
        white: "#e5e5e5",
        brightBlack: "#666666",
        brightRed: "#cd3131",
        brightGreen: "#14ce14",
        brightYellow: "#b5ba00",
        brightBlue: "#0451a5",
        brightMagenta: "#bc05bc",
        brightCyan: "#0598bc",
        brightWhite: "#ffffff",
    };
}

function terminalWebSocketUrl(sessionId: string): string {
    const url = new URL(
        `/api/sessions/${encodeURIComponent(sessionId)}/terminal/ws`,
        window.location.origin,
    );
    url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    return url.toString();
}

function rawBinaryBytes(data: string): Uint8Array {
    return Uint8Array.from(data, (character) => character.charCodeAt(0) & 0xff);
}

function closeDescription(code: number, reason: string): string {
    const detail = reason.trim() ? ` ${reason.trim()}` : "";
    switch (code) {
        case 1000:
            return "Terminal detached.";
        case 4404:
            return `Session no longer exists.${detail}`;
        case 4409:
            return `Terminal state changed; reconnect or resume it.${detail}`;
        case 4429:
            return `Terminal connection reached its queue limit.${detail}`;
        case 4503:
            return `Terminal service is unavailable.${detail}`;
        case 1011:
            return `Terminal connection failed internally.${detail}`;
        default:
            return `Terminal connection closed.${detail}`;
    }
}

function parseServerFrame(data: string): TerminalServerFrame | null {
    try {
        const frame = JSON.parse(data) as Partial<TerminalServerFrame>;
        if (frame.type === "ready" || frame.type === "exited" || frame.type === "error") {
            return frame as TerminalServerFrame;
        }
    } catch {
        return null;
    }
    return null;
}

function dialogHasFocus(): boolean {
    const activeElement = document.activeElement;
    if (activeElement === null) {
        return false;
    }
    return Array.from(
        document.querySelectorAll<HTMLElement>('[role="dialog"], .fui-DialogSurface'),
    ).some((dialog) => dialog.contains(activeElement));
}

export default function Terminal({
    sessionId,
    darkMode,
    reconnectKey,
    scrollbackMode = false,
    onAttachmentChange,
    onConnectionStateChange,
    onLifecycleStatus,
    onScrollbackModeChange = () => {},
}: TerminalProps) {
    const containerRef = useRef<HTMLDivElement>(null);
    const terminalRef = useRef<XtermTerminal | null>(null);
    const darkModeRef = useRef(darkMode);
    const scrollbackModeRef = useRef(scrollbackMode);
    const callbacksRef = useRef({
        onAttachmentChange,
        onConnectionStateChange,
        onLifecycleStatus,
        onScrollbackModeChange,
    });
    const [statusText, setStatusText] = useState("Connecting to terminal…");
    const [rendererText, setRendererText] = useState("");

    useEffect(() => {
        callbacksRef.current = {
            onAttachmentChange,
            onConnectionStateChange,
            onLifecycleStatus,
            onScrollbackModeChange,
        };
    }, [
        onAttachmentChange,
        onConnectionStateChange,
        onLifecycleStatus,
        onScrollbackModeChange,
    ]);

    useEffect(() => {
        scrollbackModeRef.current = scrollbackMode;
        const terminal = terminalRef.current;
        if (terminal === null) {
            return;
        }
        if (scrollbackMode) {
            terminal.scrollToTop();
        } else {
            terminal.scrollToBottom();
            terminal.focus();
        }
    }, [scrollbackMode]);

    useEffect(() => {
        darkModeRef.current = darkMode;
        const terminal = terminalRef.current;
        if (terminal !== null) {
            terminal.options.theme = terminalTheme(darkMode);
        }
    }, [darkMode]);

    useEffect(() => {
        const container = containerRef.current;
        if (container === null) {
            return;
        }

        let disposed = false;
        let socket: WebSocket | null = null;
        let socketReady = false;
        let terminalExited = false;
        let reconnectAttempts = 0;
        let reconnectTimer: number | null = null;
        let rendererTimer: number | null = null;
        let lastSentDimensions: { cols: number; rows: number } | null = null;
        let webglAddon: WebglAddon | null = null;
        let webglContextLoss: IDisposable | null = null;

        const terminal = new XtermTerminal({
            allowProposedApi: true,
            convertEol: false,
            cursorBlink: true,
            fontFamily:
                '"Cascadia Code", "Cascadia Mono", "SFMono-Regular", Consolas, '
                + '"Liberation Mono", "Noto Color Emoji", "Segoe UI Emoji", monospace',
            fontSize: 14,
            scrollback: 100_000,
            theme: terminalTheme(darkModeRef.current),
        });
        terminalRef.current = terminal;
        const fitAddon = new FitAddon();
        const unicodeAddon = new Unicode11Addon();
        terminal.loadAddon(fitAddon);
        terminal.loadAddon(unicodeAddon);
        terminal.unicode.activeVersion = "11";
        terminal.open(container);
        terminal.attachCustomKeyEventHandler((event) => {
            if (!scrollbackModeRef.current) {
                return true;
            }
            if (event.type !== "keydown") {
                return false;
            }
            if (event.key === "q" || event.key === "Escape") {
                callbacksRef.current.onScrollbackModeChange(false);
                return false;
            }
            if (event.key === "PageUp") {
                terminal.scrollPages(-1);
            } else if (event.key === "PageDown") {
                terminal.scrollPages(1);
            } else if (event.key === "Home") {
                terminal.scrollToTop();
            } else if (event.key === "End") {
                terminal.scrollToBottom();
            } else if (event.key === "ArrowUp") {
                terminal.scrollLines(-1);
            } else if (event.key === "ArrowDown") {
                terminal.scrollLines(1);
            }
            return false;
        });

        try {
            webglAddon = new WebglAddon();
            terminal.loadAddon(webglAddon);
            webglContextLoss = webglAddon.onContextLoss(() => {
                webglContextLoss?.dispose();
                webglContextLoss = null;
                webglAddon?.dispose();
                webglAddon = null;
                setRendererText("WebGL context lost. Using the built-in terminal renderer.");
            });
        } catch {
            webglAddon?.dispose();
            webglAddon = null;
            rendererTimer = window.setTimeout(() => {
                if (!disposed) {
                    setRendererText(
                        "WebGL is unavailable. Using the built-in terminal renderer.",
                    );
                }
            }, 0);
        }

        const setConnectionState = (
            state: TerminalConnectionState,
            message: string,
        ) => {
            if (disposed) {
                return;
            }
            setStatusText(message);
            callbacksRef.current.onConnectionStateChange(state);
        };

        const sendDimensions = (force: boolean) => {
            if (
                !socketReady
                || socket === null
                || socket.readyState !== WebSocket.OPEN
            ) {
                return;
            }
            const next = { cols: terminal.cols, rows: terminal.rows };
            if (
                !force
                && lastSentDimensions?.cols === next.cols
                && lastSentDimensions.rows === next.rows
            ) {
                return;
            }
            lastSentDimensions = next;
            socket.send(JSON.stringify({ type: "resize", ...next }));
        };

        const fit = (forceSend = false) => {
            if (disposed) {
                return;
            }
            const previous = { cols: terminal.cols, rows: terminal.rows };
            try {
                fitAddon.fit();
            } catch {
                return;
            }
            const changed = previous.cols !== terminal.cols || previous.rows !== terminal.rows;
            if (forceSend || changed) {
                sendDimensions(forceSend);
            }
        };

        const resizeObserver = new ResizeObserver(() => fit());
        resizeObserver.observe(container);
        fit();

        const sendBytes = (bytes: Uint8Array) => {
            if (socketReady && socket?.readyState === WebSocket.OPEN) {
                const copy = new Uint8Array(bytes.byteLength);
                copy.set(bytes);
                socket.send(copy.buffer);
            }
        };
        const dataDisposable = terminal.onData((data) => {
            sendBytes(new TextEncoder().encode(data));
        });
        const binaryDisposable = terminal.onBinary((data) => {
            sendBytes(rawBinaryBytes(data));
        });

        const handleReady = (frame: TerminalReadyFrame) => {
            socketReady = true;
            terminalExited = false;
            reconnectAttempts = 0;
            lastSentDimensions = null;
            callbacksRef.current.onAttachmentChange({
                attachmentId: frame.attachment_id,
                cols: frame.cols,
                rows: frame.rows,
                terminal: frame.terminal,
            });
            callbacksRef.current.onLifecycleStatus(frame.terminal);
            setConnectionState("ready", "Terminal connected.");
            fit(true);
            if (!dialogHasFocus()) {
                terminal.focus();
            }
        };

        const scheduleReconnect = (code: number, reason: string) => {
            const retryable = RETRYABLE_CLOSE_CODES.has(code);
            if (!retryable || reconnectAttempts >= MAX_RECONNECT_ATTEMPTS) {
                setConnectionState(
                    code === 4404 || code === 4409 ? "error" : "disconnected",
                    closeDescription(code, reason),
                );
                return;
            }
            const delay = Math.min(
                RECONNECT_BASE_MS * (2 ** reconnectAttempts),
                RECONNECT_MAX_MS,
            );
            reconnectAttempts += 1;
            setConnectionState(
                "reconnecting",
                `${closeDescription(code, reason)} Reconnecting in ${
                    Math.ceil(delay / 1_000)
                } second${delay > 1_000 ? "s" : ""}…`,
            );
            reconnectTimer = window.setTimeout(connect, delay);
        };

        const handleBinaryOutput = (data: ArrayBuffer | ArrayBufferView) => {
            const bytes = data instanceof ArrayBuffer
                ? new Uint8Array(data)
                : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
            terminal.write(bytes);
        };

        const connect = () => {
            if (disposed) {
                return;
            }
            setConnectionState(
                reconnectAttempts === 0 ? "connecting" : "reconnecting",
                reconnectAttempts === 0
                    ? "Connecting to terminal…"
                    : "Reconnecting to terminal…",
            );
            let nextSocket: WebSocket;
            try {
                nextSocket = new WebSocket(terminalWebSocketUrl(sessionId));
            } catch {
                scheduleReconnect(1006, "WebSocket could not be created.");
                return;
            }
            socket = nextSocket;
            nextSocket.binaryType = "arraybuffer";
            nextSocket.onopen = () => {
                setStatusText("Attaching to terminal…");
            };
            nextSocket.onmessage = (event) => {
                if (disposed || socket !== nextSocket) {
                    return;
                }
                if (typeof event.data === "string") {
                    const frame = parseServerFrame(event.data);
                    if (frame?.type === "ready") {
                        handleReady(frame);
                    } else if (frame?.type === "exited") {
                        terminalExited = true;
                        socketReady = false;
                        callbacksRef.current.onAttachmentChange(null);
                        callbacksRef.current.onLifecycleStatus(frame.terminal);
                        setConnectionState(
                            "exited",
                            frame.exit_status === null
                                ? "Terminal exited."
                                : `Terminal exited with status ${frame.exit_status}.`,
                        );
                    } else if (frame?.type === "error") {
                        setConnectionState("error", frame.message);
                    }
                    return;
                }
                if (event.data instanceof ArrayBuffer || ArrayBuffer.isView(event.data)) {
                    handleBinaryOutput(event.data);
                } else if (event.data instanceof Blob) {
                    void event.data.arrayBuffer().then((data) => {
                        if (!disposed && socket === nextSocket) {
                            handleBinaryOutput(data);
                        }
                    });
                }
            };
            nextSocket.onerror = () => {
                if (!socketReady) {
                    setStatusText("Terminal connection failed before attachment.");
                }
            };
            nextSocket.onclose = (event) => {
                if (disposed || socket !== nextSocket) {
                    return;
                }
                socketReady = false;
                socket = null;
                callbacksRef.current.onAttachmentChange(null);
                if (terminalExited) {
                    return;
                }
                scheduleReconnect(event.code, event.reason);
            };
        };

        connect();

        return () => {
            disposed = true;
            callbacksRef.current.onAttachmentChange(null);
            if (reconnectTimer !== null) {
                window.clearTimeout(reconnectTimer);
            }
            if (rendererTimer !== null) {
                window.clearTimeout(rendererTimer);
            }
            resizeObserver.disconnect();
            dataDisposable.dispose();
            binaryDisposable.dispose();
            webglContextLoss?.dispose();
            webglAddon?.dispose();
            if (
                socket !== null
                && (
                    socket.readyState === WebSocket.CONNECTING
                    || socket.readyState === WebSocket.OPEN
                )
            ) {
                socket.onclose = null;
                socket.close(1000, "view detached");
            }
            socket = null;
            terminal.dispose();
            terminalRef.current = null;
        };
    }, [reconnectKey, sessionId]);

    return (
        <div className="terminal-shell">
            <div
                aria-label="Interactive CLI terminal"
                className="terminal-canvas"
                ref={containerRef}
            />
            <div aria-atomic="true" aria-live="polite" className="terminal-status" role="status">
                {statusText}
            </div>
            {rendererText !== "" && (
                <div aria-live="polite" className="visually-hidden" role="status">
                    {rendererText}
                </div>
            )}
        </div>
    );
}
