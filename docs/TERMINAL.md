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
the WebSocket to `agent_host`. `agent_host` then connects to the authenticated
`/terminal` endpoint on the existing private `kernel_host` port. `kernel_host`
creates and owns a native PTY with `/workspace` as its working directory.

No terminal port is published to the host or browser: the endpoint shares the
private kernel API listener already used by `agent_host`. Closing the pane
closes the proxied WebSockets, terminates the PTY process groups, and reaps the
login shell. Reopening the pane creates a fresh shell.

## WebSocket protocol

- Browser-to-server terminal input is sent as binary UTF-8 data.
- Server-to-browser terminal output is sent as binary data.
- Terminal resize messages are JSON text:

```json
{"type":"resize","cols":120,"rows":32}
```

Terminal dimensions are limited to 500 columns by 300 rows.

## Security

Browser WebSocket upgrades are accepted when the `Origin` header is same-origin
with the request authority (the `X-Forwarded-Host` or `Host` header), which is
always the case for the Web UI because it proxies `/api` from its own origin.
This works for plain-HTTP local deployments on any host name, LAN address, or
port without configuration. Cross-origin upgrades must be listed in
`CLIENT_SERVICE_CORS_ALLOWED_ORIGINS`. Requests without an `Origin` header
remain available to trusted service and CLI clients. `agent_host` rejects
browser-originated terminal upgrades so browser access must pass through
`client_service` and its client-session lookup. Each kernel container receives
a random terminal bearer token at creation. `agent_host` stores that token and
adds it to the private kernel WebSocket handshake; it is never returned to the
browser or published as part of the session summary.
