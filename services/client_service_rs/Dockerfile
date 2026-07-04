# syntax=docker/dockerfile:1

FROM docker.io/library/rust:1.96-slim-bookworm AS builder

ARG RUST_BUILD_PROFILE=release

WORKDIR /usr/src/agentspace

COPY Cargo.toml Cargo.lock ./
COPY services/agent_host_rs services/agent_host_rs
COPY services/client_service_rs services/client_service_rs
RUN --mount=type=cache,id=agentspace-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=agentspace-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    case "$RUST_BUILD_PROFILE" in \
        release) cargo build --locked --release --package client_service_rs --bin client_service_rs \
            && cp target/release/client_service_rs /usr/local/bin/client_service_rs ;; \
        debug) cargo build --locked --package client_service_rs --bin client_service_rs \
            && cp target/debug/client_service_rs /usr/local/bin/client_service_rs ;; \
        *) echo "unsupported RUST_BUILD_PROFILE=$RUST_BUILD_PROFILE" >&2; exit 1 ;; \
    esac

FROM docker.io/library/debian:bookworm-slim AS runtime

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/local/bin/client_service_rs /usr/local/bin/client_service_rs

ENV CLIENT_SERVICE_HOST=0.0.0.0 \
    CLIENT_SERVICE_PORT=8002

EXPOSE 8002

CMD ["/usr/local/bin/client_service_rs"]

ARG AGENTSPACE_VERSION=dev
ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}
