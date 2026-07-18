# syntax=docker/dockerfile:1

FROM docker.io/library/rust:1.96-slim-bookworm AS builder

ARG RUST_BUILD_PROFILE=release

WORKDIR /usr/src/agentspace

COPY Cargo.toml Cargo.lock ./
COPY services/agent_host_rs services/agent_host_rs
COPY services/client_service_rs services/client_service_rs
COPY services/memory_rs services/memory_rs
RUN --mount=type=cache,id=agentspace-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=agentspace-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    case "$RUST_BUILD_PROFILE" in \
        release) cargo build --locked --release --package agent_host_rs --bin agent_host_rs \
            && cp target/release/agent_host_rs /usr/local/bin/agent_host_rs ;; \
        debug) cargo build --locked --package agent_host_rs --bin agent_host_rs \
            && cp target/debug/agent_host_rs /usr/local/bin/agent_host_rs ;; \
        *) echo "unsupported RUST_BUILD_PROFILE=$RUST_BUILD_PROFILE" >&2; exit 1 ;; \
    esac

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

ARG AGENTSPACE_VERSION=dev
ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}
