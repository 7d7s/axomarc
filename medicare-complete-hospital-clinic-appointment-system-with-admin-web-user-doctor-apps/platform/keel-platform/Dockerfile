# syntax=docker/dockerfile:1
# Kelson runtime - non-root, minimal image.
# Build: docker build -t kelson:latest -f platform/keel-platform/Dockerfile platform/keel-platform
#
# The image is checked against the runtime it ships by
# `the_container_image_agrees_with_the_runtime` (the console's suite): the
# toolchain, the port, the config and the module artifacts must agree, or
# that case fails. A Dockerfile that drifts from the runtime is a broken
# release, so the drift is a test failure rather than a deployment incident.

# ---- Build stage ----
# The Rust version MUST match rust-toolchain.toml: the workspace is pinned,
# and a different compiler is a different artifact.
FROM rust:1.92-slim-bookworm AS builder
WORKDIR /app
# Install build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
# Copy source
COPY . .
# Build with pinned env
ENV CARGO_BUILD_JOBS=1
ENV CARGO_INCREMENTAL=0
RUN cargo build --release --package kelson-demo 2>&1 | tail -5

# ---- Runtime stage ----
FROM debian:bookworm-slim AS runtime
# curl is here for the healthcheck only; there is no shell tooling beyond it.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl && rm -rf /var/lib/apt/lists/* && \
    useradd --create-home --shell /bin/bash kelson
WORKDIR /app
# Copy binary
COPY --from=builder /app/target/release/kelson-demo /app/kelson-demo
# The config: the boot reads `config/dev.toml` from the working directory,
# so without it the host refuses to start.
COPY --from=builder /app/config /app/config
# The package index and the WASM module artifacts it points at. The paths in
# packages.toml resolve against the WORKING DIRECTORY, so both the index and
# every artifact it names must be present or the boot finds no module.
COPY --from=builder /app/packages.toml /app/packages.toml
COPY --from=builder /app/applications /app/applications
COPY --from=builder /app/testing /app/testing
# Non-root user
USER kelson
# The port the config listens on (config/dev.toml: `listen = 127.0.0.1:8180`).
# MNC-039: the committed dev config binds loopback, which no Service/Ingress/
# probe outside the pod can reach. The image overrides the bind to `0.0.0.0`
# so it is reachable, and declares a TLS-terminating proxy (MNC-006) so the
# boot's transport-posture guard allows the public bind (TLS terminates at the
# ingress; the pod speaks plain HTTP internally). A real deployment fronting
# the pod without a TLS proxy would set `KEEL_TERMINATING_PROXY=0` and terminate
# TLS here instead.
ENV KEEL_LISTEN=0.0.0.0:8180
ENV KEEL_TERMINATING_PROXY=1
EXPOSE 8180
# The start period covers the WASM JIT: the boot compiles every guest
# artifact before it binds, which takes tens of seconds.
HEALTHCHECK --interval=30s --timeout=5s --start-period=90s --retries=3 \
    CMD curl -f http://127.0.0.1:8180/healthz || exit 1
ENTRYPOINT ["/app/kelson-demo"]
