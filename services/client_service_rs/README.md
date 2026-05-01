# client_service_rs

Rust implementation of the AgentSpace `client_service` API. This is the active
and default client-service implementation.

## Run locally

Start the service from this directory:

```sh
./run-service.sh
```

The script runs `cargo run` with the default client-service bind address:

- `CLIENT_SERVICE_HOST` defaults to `0.0.0.0`
- `CLIENT_SERVICE_PORT` defaults to `8002`
- `CLIENT_SERVICE_AGENT_HOST_BASE_URL` defaults to `http://127.0.0.1:8001`
- `CLIENT_SERVICE_GIT_AGENT_BASE_URL` defaults to `http://127.0.0.1:8004`
  locally and `http://git-agent:8004` when running in a container

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
- `CLIENT_SERVICE_GIT_AGENT_BASE_URL=http://git-agent:8004`

The root compose stack builds and runs this Rust service by default:

```sh
just stack-up
```

In compose, the dashboard talks to this service through the usual
`client-service:8002` DNS name.

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
