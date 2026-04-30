# client_service_rs TODO

## Current state

`client_service_rs` is an initial Rust port milestone for
`services/client_service`. It is a standalone Axum service intended to match the
Python FastAPI service API surface closely enough for incremental replacement
work.

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

## Remaining work

### In progress in the current porting pass

- Add durable SQLite persistence for Rust service state.
- Add route-level contract coverage for Python-compatible response shapes and
  status codes.
- Add stub `agent_host` integration coverage for proxied routes and session
  environment merging.
- Improve harness listing and gateway schema parity.
- Add operational Docker/check wiring where it is useful without replacing the
  Python service defaults.

### Backlog

1. Add durable SQLite persistence compatible with the Python service tables or
   define and document a migration path.
2. Compare every Rust response body against the Python FastAPI service with
   contract tests, especially update/patch semantics and error payload details.
3. Expand `/harnesses` parity once the Rust service can discover all registered
   harnesses instead of returning only the currently wired default.
4. Improve active-turn lifecycle parity for streaming reconnects via
   `/sessions/{session_id}/turns/{turn_id}/stream`.
5. Persist and replay completed turn metadata if reconnect behavior needs to
   survive process restarts.
6. Implement full gateway schema parity for
   `/gateway-types/{gateway_type}/schema` instead of the current simplified
   schema response.
7. Add integration tests with a stub or real `agent_host` covering:
   - session creation environment merging
   - message send and stream behavior
   - kernel list/log/container-log routes
   - skill proxy routes
   - gateway start/stop/log routes
8. Add Docker and compose integration once the Rust service is ready to run as a
   drop-in replacement in the stack.
9. Decide whether `client_service_rs` should be wired into top-level repo checks
   such as `just check`.
10. Add performance and concurrency tests for simultaneous sessions, message
    streams, and gateway operations.

## Notes for the next porting pass

- Keep the Python `services/client_service` implementation available as the
  source of truth until contract tests show parity.
- Run the Rust crate checks after each change from `services/client_service_rs`.
- Prefer adding parity tests before changing behavior so differences from the
  Python service are intentional and visible.
