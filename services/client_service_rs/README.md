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

## Run with a container

Build the Rust service image from the repository root:

```sh
podman build -f services/client_service_rs/Dockerfile -t agentspace-client-service-rs:latest services/client_service_rs
```

Use `CONTAINER_RUNTIME=docker` with the `just` recipe if you prefer Docker:

```sh
just client-service-rs-image
```

The container defaults match `run-service.sh`:

- `CLIENT_SERVICE_HOST=0.0.0.0`
- `CLIENT_SERVICE_PORT=8002`
- `CLIENT_SERVICE_AGENT_HOST_BASE_URL=http://127.0.0.1:8001`

When running on the repo compose network, point the Rust service at the existing
`agent-host` service without changing the default Python compose wiring:

```sh
podman compose -f compose.yaml -f compose.client-service-rs.yaml up -d --build client-service
```

The root compose file can also be flipped to the Rust service with interpolation
values:

```sh
podman compose --env-file compose.client-service-rs.env -f compose.yaml up -d --build client-service
```

Or use the equivalent repository-root recipe:

```sh
just stack-up-client-service-rs
```

Add `webui` to that command if you also want the dashboard to talk to the Rust
client service through the usual `client-service:8002` compose DNS name.

## Validate

Run the Rust crate checks from this directory:

```sh
cargo fmt --check
cargo test --quiet
cargo clippy --all-targets --all-features
```

Or from the repository root:

```sh
just client-service-rs-check
```
