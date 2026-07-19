# syntax=docker/dockerfile:1

FROM scratch AS memory-rs
COPY services/memory_rs /

FROM scratch AS session-tools-rs
COPY services/session_tools_rs /

FROM docker.io/library/rust:1.96-slim-bookworm AS builder

ARG RUST_BUILD_PROFILE=release

WORKDIR /usr/src/agentspace

COPY Cargo.toml Cargo.lock ./
COPY services/agent_host_rs services/agent_host_rs
COPY services/client_service_rs services/client_service_rs
COPY --from=memory-rs . services/memory_rs
COPY --from=session-tools-rs . services/session_tools_rs
RUN --mount=type=cache,id=agentspace-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=agentspace-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    case "$RUST_BUILD_PROFILE" in \
        release) cargo build --locked --release --package memory_rs --bin memory \
            && cp target/release/memory /usr/local/bin/memory ;; \
        debug) cargo build --locked --package memory_rs --bin memory \
            && cp target/debug/memory /usr/local/bin/memory ;; \
        *) echo "unsupported RUST_BUILD_PROFILE=$RUST_BUILD_PROFILE" >&2; exit 1 ;; \
    esac

FROM docker.io/library/debian:bookworm-slim AS runtime

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/local/bin/memory /usr/local/bin/memory

ENV AGENTSPACE_MEMORY_DIR=/var/lib/agentspace/memory

EXPOSE 8005

ENTRYPOINT ["/usr/local/bin/memory"]
CMD ["--serve", "--host", "0.0.0.0", "--port", "8005"]

ARG AGENTSPACE_VERSION=dev
ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}
