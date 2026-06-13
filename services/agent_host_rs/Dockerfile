# syntax=docker/dockerfile:1

FROM docker.io/library/rust:1.96-slim-bookworm AS builder

WORKDIR /usr/src/agent_host_rs

COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && printf 'pub fn placeholder() {}\n' > src/lib.rs \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo fetch --locked

COPY . .
RUN cargo build --locked --release

FROM docker.io/library/debian:bookworm-slim AS runtime

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/src/agent_host_rs/target/release/agent_host_rs /usr/local/bin/agent_host_rs

ENV AGENT_HOST_HOST=0.0.0.0 \
    AGENT_HOST_PORT=8001

EXPOSE 8001

CMD ["/usr/local/bin/agent_host_rs"]
