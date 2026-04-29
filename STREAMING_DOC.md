# Persisted Server-Side Streaming

## Goal

Client Service should own agent turns. Browser or CLI clients may disconnect while an agent response is in flight, but the server must keep reading the upstream agent stream, accumulate the complete assistant response, and persist the conversation state. When a client reconnects later, it should see the server's current conversation view and be able to attach to any still-running turn.

## Existing Problem

Before this work, `/sessions/{session_id}/messages/stream` tied the upstream agent stream directly to the HTTP response. If the browser refreshed or navigated away, the response generator was closed, which closed the upstream `agent_host` stream. Client Service only stored the events already received, so a response could be truncated or lost.

## Design

### Persistence

Add a `SessionStore` abstraction with in-memory and SQLite implementations. It stores:

- client sessions
- persisted chat messages
- per-message tool call metadata

The app will wire `SqliteSessionStore` whenever `CLIENT_SERVICE_DB_PATH` is configured, alongside the existing SQLite-backed agent, kernel config, gateway, and connection stores. The default no-DB path remains in-memory for tests and local ephemeral runs.

### Server-owned turns

A send request creates a server-side turn:

1. Persist the user message immediately.
2. Create an assistant placeholder message with empty content.
3. Start an asyncio task that reads `agent_host.stream_message(...)` to completion, independent of any browser stream connection.
4. Apply each kernel event to an in-memory turn accumulator and persist the assistant message after visible updates.
5. Broadcast every event and the final payload to attached stream subscribers.

HTTP stream clients no longer own the upstream stream. They attach to the server-side turn. If the client disconnects, only that subscriber is removed; the upstream task continues.

### Reconnect behavior

Session details include the persisted messages. While a turn is running, the assistant placeholder is part of the messages array and is updated as chunks arrive. The web UI can therefore refresh and immediately render the current partial assistant content from `GET /sessions/{session_id}`.

Session summaries/details include an `active_turn` object while a turn is running:

- `turn_id`
- `user_message_id`
- `assistant_message_id`
- `status`

The existing `POST /sessions/{session_id}/messages/stream` endpoint starts a new turn and subscribes the caller to it. The reconnect endpoint is `GET /sessions/{session_id}/turns/{turn_id}/stream`; it attaches to a process-local running turn without resubmitting the prompt.

The web UI uses both paths:

- normal send: call `POST /messages/stream`, render local optimistic bubbles, and replace them with the persisted final payload;
- refresh/reconnect: fetch session detail, see `active_turn`, render the persisted partial assistant message, then attach with `GET /turns/{turn_id}/stream` for future chunks and the final payload.

## Implementation Notes

- Turn state is process-local: active asyncio tasks and subscriber queues do not survive a Client Service process restart.
- Conversation history is persisted when a SQLite DB is configured.
- In-memory mode keeps the same persistence lifetime as before, but with server-owned streaming semantics.
- Events are stored in the in-flight turn object for final stream payloads; message content/tool calls are the durable conversation surface.

## Open Follow-Ups

- Persist raw per-turn event logs if clients need replay of every event after reconnect, not just the rendered assistant message.
- Consider rehydrating interrupted active turns after Client Service restart if upstream kernels can expose resumable in-flight state.
