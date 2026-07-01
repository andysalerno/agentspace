# syntax=docker/dockerfile:1

FROM docker.io/library/rust:1.96-slim-bookworm AS builder

WORKDIR /usr/src/agentspace

COPY Cargo.toml Cargo.lock ./
COPY services/agent_host_rs/Cargo.toml services/agent_host_rs/Cargo.toml
COPY services/client_service_rs/Cargo.toml services/client_service_rs/Cargo.toml
RUN mkdir -p services/agent_host_rs/src services/client_service_rs/src \
    && printf 'pub fn placeholder() {}\n' > services/agent_host_rs/src/lib.rs \
    && printf 'fn main() {}\n' > services/agent_host_rs/src/main.rs \
    && printf 'pub fn placeholder() {}\n' > services/client_service_rs/src/lib.rs \
    && printf 'fn main() {}\n' > services/client_service_rs/src/main.rs \
    && cargo fetch --locked --manifest-path services/client_service_rs/Cargo.toml

COPY services/client_service_rs services/client_service_rs
RUN cargo build --locked --release --manifest-path services/client_service_rs/Cargo.toml

FROM docker.io/library/debian:bookworm-slim AS runtime

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/src/agentspace/target/release/client_service_rs /usr/local/bin/client_service_rs

ENV CLIENT_SERVICE_HOST=0.0.0.0 \
    CLIENT_SERVICE_PORT=8002

EXPOSE 8002

CMD ["/usr/local/bin/client_service_rs"]
