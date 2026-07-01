# syntax=docker/dockerfile:1

FROM docker.io/library/rust:1.96-slim-bookworm AS builder

WORKDIR /usr/src/agentspace

COPY Cargo.toml Cargo.lock ./
COPY services/agent_host_rs/Cargo.toml services/agent_host_rs/Cargo.toml
COPY services/client_service_rs/Cargo.toml services/client_service_rs/Cargo.toml
RUN --mount=type=cache,id=agentspace-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=agentspace-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    mkdir -p services/agent_host_rs/src services/client_service_rs/src \
    && printf 'pub fn placeholder() {}\n' > services/agent_host_rs/src/lib.rs \
    && printf 'fn main() {}\n' > services/agent_host_rs/src/main.rs \
    && printf 'pub fn placeholder() {}\n' > services/client_service_rs/src/lib.rs \
    && printf 'fn main() {}\n' > services/client_service_rs/src/main.rs \
    && cargo build --locked --release --workspace

COPY services/agent_host_rs services/agent_host_rs
RUN --mount=type=cache,id=agentspace-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=agentspace-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    cargo build --locked --release --package agent_host_rs --bin agent_host_rs \
    && cp target/release/agent_host_rs /usr/local/bin/agent_host_rs

FROM docker.io/library/debian:bookworm-slim AS runtime

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/local/bin/agent_host_rs /usr/local/bin/agent_host_rs

ENV AGENT_HOST_HOST=0.0.0.0 \
    AGENT_HOST_PORT=8001

EXPOSE 8001

CMD ["/usr/local/bin/agent_host_rs"]
