# Interactive terminal

The chat view includes an integrated terminal for the selected agent session.
It uses [xterm.js](https://xtermjs.org/), the terminal renderer used by VS Code,
and opens in a bottom pane without leaving the conversation.

## Architecture

The browser connects to:

```text
GET /api/sessions/{client_session_id}/terminal?cols=120&rows=32
```

The connection is a WebSocket routed through the normal Web UI nginx proxy.
`client_service` resolves the client session to its kernel session and proxies
the WebSocket to `agent_host`. `agent_host` starts an attached Docker Exec TTY
inside the existing kernel container with `/workspace` as its working directory.

This design does not expose SSH, add a daemon to the kernel image, or publish
another container port. Closing the pane closes the WebSocket and its Docker
Exec input stream; reopening the pane creates a fresh shell.

## WebSocket protocol

- Browser-to-server terminal input is sent as binary UTF-8 data.
- Server-to-browser terminal output is sent as binary data.
- Terminal resize messages are JSON text:

```json
{"type":"resize","cols":120,"rows":32}
```

Terminal dimensions are limited to 500 columns by 300 rows.

## Security

Browser WebSocket origins are checked against
`CLIENT_SERVICE_CORS_ALLOWED_ORIGINS`. Requests without an `Origin` header
remain available to trusted service and CLI clients. `agent_host` rejects
browser-originated terminal upgrades so browser access must pass through
`client_service` and its client-session lookup.
