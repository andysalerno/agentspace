# client_service_rs TODO

## Current state

`client_service_rs` is a Rust port milestone for `services/client_service`. It
is a standalone Axum service intended to match the Python FastAPI service API
surface closely enough for incremental replacement work.

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
- Added opt-in Docker and compose wiring plus `just client-service-rs-check`,
  `just client-service-rs-image`, and `just stack-up-client-service-rs`.

## Remaining work

1. Compare every Rust response body against a live Python FastAPI service with
   end-to-end contract tests; current tests cover important route contracts and
   proxy behavior but do not run both implementations side by side.
2. Improve active-turn lifecycle parity for streaming reconnects via
   `/sessions/{session_id}/turns/{turn_id}/stream`.
3. Persist and replay completed turn metadata if reconnect behavior needs to
   survive process restarts.
4. Continue refining gateway schema parity if new gateway implementations add
   dynamic settings beyond the current echo/Discord schema responses.
5. Decide whether `client_service_rs` should be wired into top-level repo checks
   such as `just check`; it currently has a dedicated `just client-service-rs-check`.
6. Add performance and concurrency tests for simultaneous sessions, message
    streams, and gateway operations.
7. Add a planned cutover path for making `client_service_rs` the default
   `client_service` in the full stack once parity is proven.

## Notes for the next porting pass

- Keep the Python `services/client_service` implementation available as the
  source of truth until contract tests show parity.
- Run the Rust crate checks after each change from `services/client_service_rs`.
- Prefer adding parity tests before changing behavior so differences from the
  Python service are intentional and visible.
