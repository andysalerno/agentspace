# client_service_rs

Rust port milestone for the AgentSpace `client_service` API.

## Run locally

Start the service from this directory:

```sh
./run-service.sh
```

The script runs `cargo run` with the same default bind address as the Python
service:

- `CLIENT_SERVICE_HOST` defaults to `0.0.0.0`
- `CLIENT_SERVICE_PORT` defaults to `8002`
- `CLIENT_SERVICE_AGENT_HOST_BASE_URL` defaults to `http://127.0.0.1:8001`

`GET /healthz` works without `agent_host`; session and kernel operations expect
an `agent_host` instance at `CLIENT_SERVICE_AGENT_HOST_BASE_URL`.

## Validate

Run the Rust crate checks from this directory:

```sh
cargo fmt --check
cargo test --quiet
cargo clippy --all-targets --all-features
```
