# client_service_rs TODO

## Current state

`client_service_rs` is the active Rust implementation of the AgentSpace
`client_service` API. It is a standalone Axum service and the default
client-service used by the compose stack.

The crate currently builds and validates with:

```sh
cargo fmt --check
cargo test --quiet
cargo clippy --all-targets --all-features
```

The service can be started from this directory with:

```sh
./run-service.sh
```

## Completed

- Created a standalone Rust crate under `services/client_service_rs`.
- Added strict Rust and Clippy lint configuration.
- Added Axum server startup, CORS, tracing, and environment-based bind
  configuration.
- Implemented core models matching Python response shapes for agents, sessions,
  messages, tool calls, connections, kernel configs, and gateways.
- Implemented validation helpers for IDs, harness names, gateway types, and
  environment variable parsing.
- Implemented async-friendly in-memory stores for current API behavior.
- Implemented a `reqwest`-based `agent_host` client, including NDJSON message
  streaming helpers and proxy methods for sessions, kernels, skills, gateways,
  logs, and service info.
- Implemented the main HTTP route surface:
  - `/healthz`, `/info`, `/harnesses`
  - `/kernel-configs`
  - `/connections`
  - `/agents`
  - `/sessions` and session message routes
  - `/kernels`
  - `/skills`
  - `/gateway-types`
  - `/gateways`
- Added route and unit tests for the implemented in-memory behavior and upstream
  client helpers.
- Added `README.md`, `.gitignore`, and `run-service.sh`.
- Added durable SQLite persistence selected by `CLIENT_SERVICE_DB_PATH`, using
  table and column names compatible with the Python service where practical.
- Added route-level contract tests for Python-compatible response shapes and
  status codes.
- Added stub `agent_host` integration tests for session environment merging,
  message send/stream behavior, kernels, logs, skills, and gateways.
- Expanded `/harnesses` to all modeled harnesses and improved gateway schema
  responses for echo and Discord gateways.
- Added Docker wiring plus `just client-service-rs-check` and
  `just client-service-rs-image`.
- Promoted the Rust service to the root compose default and removed the old
  Python implementation.

## Remaining work

1. Improve active-turn lifecycle parity for streaming reconnects via
   `/sessions/{session_id}/turns/{turn_id}/stream`.
2. Persist and replay completed turn metadata if reconnect behavior needs to
   survive process restarts.
3. Continue refining gateway schema parity if new gateway implementations add
   dynamic settings beyond the current echo/Discord schema responses.
4. Decide whether `client_service_rs` should be wired into top-level repo checks
   such as `just check`; it currently has a dedicated `just client-service-rs-check`.
5. Add performance and concurrency tests for simultaneous sessions, message
    streams, and gateway operations.

## Notes for future changes

- Run the Rust crate checks after each change from `services/client_service_rs`.
- Prefer adding route contract tests before changing behavior so API differences
  are intentional and visible.
