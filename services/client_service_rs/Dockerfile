# syntax=docker/dockerfile:1

FROM docker.io/library/rust:1.95-slim-bookworm AS builder

WORKDIR /usr/src/client_service_rs

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

COPY --from=builder /usr/src/client_service_rs/target/release/client_service_rs /usr/local/bin/client_service_rs

ENV CLIENT_SERVICE_HOST=0.0.0.0 \
    CLIENT_SERVICE_PORT=8002

EXPOSE 8002

CMD ["/usr/local/bin/client_service_rs"]
