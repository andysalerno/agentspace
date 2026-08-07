import { useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import {
    ArrowClockwise20Regular,
    Dismiss20Regular,
    WindowConsole20Regular,
} from "@fluentui/react-icons";
import { terminalWebSocketUrl } from "./api";
import { Button, Tooltip } from "./fluent";
import "@xterm/xterm/css/xterm.css";

type TerminalPaneProps = {
    sessionId: string;
    onClose: () => void;
};

type ConnectionState = "connecting" | "connected" | "disconnected" | "error";

const terminalFont = '"Cascadia Code", "Cascadia Mono", "SFMono-Regular", Consolas, monospace';

function terminalTheme() {
    const dark = document.documentElement.dataset.theme === "dark";
    return dark
        ? {
            background: "#141414",
            foreground: "#d4d4d4",
            cursor: "#ffffff",
            cursorAccent: "#141414",
            selectionBackground: "#264f78",
            black: "#000000",
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
            brightWhite: "#e5e5e5",
        }
        : {
            background: "#1e1e1e",
            foreground: "#d4d4d4",
            cursor: "#ffffff",
            cursorAccent: "#1e1e1e",
            selectionBackground: "#264f78",
            black: "#000000",
            red: "#cd3131",
            green: "#0dbc79",
            yellow: "#e5e510",
            blue: "#2472c8",
            magenta: "#bc3fbc",
            cyan: "#11a8cd",
            white: "#e5e5e5",
            brightBlack: "#666666",
            brightRed: "#f14c4c",
            brightGreen: "#23d18b",
            brightYellow: "#f5f543",
            brightBlue: "#3b8eea",
            brightMagenta: "#d670d6",
            brightCyan: "#29b8db",
            brightWhite: "#e5e5e5",
        };
}

function connectionLabel(state: ConnectionState): string {
    switch (state) {
        case "connecting":
            return "Connecting";
        case "connected":
            return "Connected";
        case "disconnected":
            return "Disconnected";
        case "error":
            return "Connection error";
    }
}

export default function TerminalPane({ sessionId, onClose }: TerminalPaneProps) {
    const hostRef = useRef<HTMLDivElement>(null);
    const [connectionState, setConnectionState] = useState<ConnectionState>("connecting");
    const [reconnectKey, setReconnectKey] = useState(0);

    useEffect(() => {
        const host = hostRef.current;
        if (host === null) {
            return;
        }

        const terminal = new Terminal({
            allowProposedApi: false,
            cursorBlink: true,
            cursorStyle: "block",
            fontFamily: terminalFont,
            fontSize: 13,
            lineHeight: 1.2,
            scrollback: 10_000,
            theme: terminalTheme(),
        });
        const fitAddon = new FitAddon();
        terminal.loadAddon(fitAddon);
        terminal.open(host);

        let socket: WebSocket | null = null;
        let disposed = false;
        const encoder = new TextEncoder();

        const sendResize = () => {
            if (socket?.readyState === WebSocket.OPEN) {
                socket.send(JSON.stringify({
                    type: "resize",
                    cols: terminal.cols,
                    rows: terminal.rows,
                }));
            }
        };
        const fit = () => {
            if (host.clientWidth === 0 || host.clientHeight === 0) {
                return;
            }
            fitAddon.fit();
            sendResize();
        };

        const resizeObserver = new ResizeObserver(fit);
        resizeObserver.observe(host);
        const themeObserver = new MutationObserver(() => {
            terminal.options.theme = terminalTheme();
        });
        themeObserver.observe(document.documentElement, {
            attributes: true,
            attributeFilter: ["data-theme"],
        });

        const inputDisposable = terminal.onData((data) => {
            if (socket?.readyState === WebSocket.OPEN) {
                socket.send(encoder.encode(data));
            }
        });
        const resizeDisposable = terminal.onResize(sendResize);

        requestAnimationFrame(() => {
            if (disposed) {
                return;
            }
            fit();
            setConnectionState("connecting");
            socket = new WebSocket(
                terminalWebSocketUrl(sessionId, terminal.cols, terminal.rows),
            );
            socket.binaryType = "arraybuffer";
            socket.onopen = () => {
                if (disposed) {
                    return;
                }
                setConnectionState("connected");
                sendResize();
                terminal.focus();
            };
            socket.onmessage = (event) => {
                if (typeof event.data === "string") {
                    try {
                        const payload = JSON.parse(event.data) as {
                            type?: unknown;
                            message?: unknown;
                        };
                        if (payload.type === "error" && typeof payload.message === "string") {
                            setConnectionState("error");
                            terminal.writeln(`\r\n\x1b[31m${payload.message}\x1b[0m`);
                        }
                    } catch {
                        terminal.write(event.data);
                    }
                    return;
                }
                if (event.data instanceof ArrayBuffer) {
                    terminal.write(new Uint8Array(event.data));
                }
            };
            socket.onerror = () => {
                if (!disposed) {
                    setConnectionState("error");
                }
            };
            socket.onclose = () => {
                if (!disposed) {
                    setConnectionState((current) =>
                        current === "error" ? current : "disconnected"
                    );
                }
            };
        });

        return () => {
            disposed = true;
            resizeObserver.disconnect();
            themeObserver.disconnect();
            inputDisposable.dispose();
            resizeDisposable.dispose();
            socket?.close();
            terminal.dispose();
        };
    }, [reconnectKey, sessionId]);

    const statusLabel = connectionLabel(connectionState);
    return (
        <section aria-label="Interactive terminal" className="terminal-pane">
            <header className="terminal-pane-header">
                <div className="terminal-pane-title">
                    <WindowConsole20Regular />
                    <span>Terminal</span>
                    <span className="terminal-pane-path">/workspace</span>
                </div>
                <div className="terminal-pane-actions">
                    <span className={`terminal-connection ${connectionState}`}>
                        <span aria-hidden="true" className="terminal-connection-dot" />
                        {statusLabel}
                    </span>
                    {connectionState !== "connected" && (
                        <Tooltip content="Reconnect terminal" relationship="label">
                            <Button
                                appearance="subtle"
                                aria-label="Reconnect terminal"
                                icon={<ArrowClockwise20Regular />}
                                onClick={() => setReconnectKey((key) => key + 1)}
                                size="small"
                            />
                        </Tooltip>
                    )}
                    <Tooltip content="Close terminal" relationship="label">
                        <Button
                            appearance="subtle"
                            aria-label="Close terminal"
                            icon={<Dismiss20Regular />}
                            onClick={onClose}
                            size="small"
                        />
                    </Tooltip>
                </div>
            </header>
            <div className="terminal-host" ref={hostRef} />
        </section>
    );
}
