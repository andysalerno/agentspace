# syntax=docker/dockerfile:1

FROM scratch AS memory-rs
COPY services/memory_rs /

FROM docker.io/library/rust:1.97-slim-trixie AS builder

ARG RUST_BUILD_PROFILE=release

WORKDIR /usr/src/agentspace

COPY Cargo.toml Cargo.lock ./
COPY clients/agentspace_cli_rs clients/agentspace_cli_rs
COPY services/agent_host_rs services/agent_host_rs
COPY services/client_service_rs services/client_service_rs
COPY --from=memory-rs . services/memory_rs
RUN --mount=type=cache,id=agentspace-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=agentspace-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    case "$RUST_BUILD_PROFILE" in \
        release) cargo build --locked --release --package agentspace_cli_rs --bin agentspace \
            && cp target/release/agentspace /usr/local/bin/agentspace ;; \
        debug) cargo build --locked --package agentspace_cli_rs --bin agentspace \
            && cp target/debug/agentspace /usr/local/bin/agentspace ;; \
        *) echo "unsupported RUST_BUILD_PROFILE=$RUST_BUILD_PROFILE" >&2; exit 1 ;; \
    esac

FROM docker.io/library/debian:trixie-slim AS runtime

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /usr/local/bin/agentspace /usr/local/bin/agentspace

ENV AGENTSPACE_MEMORY_DIR=/var/lib/agentspace/memory

EXPOSE 8005

ENTRYPOINT ["/usr/local/bin/agentspace"]
CMD ["memory", "--serve", "--host", "0.0.0.0", "--port", "8005"]

ARG AGENTSPACE_VERSION=dev
ENV AGENTSPACE_VERSION=${AGENTSPACE_VERSION}
