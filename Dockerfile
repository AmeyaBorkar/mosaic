# syntax=docker/dockerfile:1
#
# Production image for `mosaic-server`. Multi-stage so the runtime carries only the
# binary and glibc — no Rust toolchain, no source. cargo-chef splits dependency
# compilation from the app build so a source-only change reuses the cached dependency
# layer (wasmtime + axum are the bulk of the build).
#
# Pinned to the workspace toolchain (rust-toolchain.toml) so the container builds Mosaic
# identically to CI. Reproducibility of the binary comes from Cargo.lock (`--locked`).

FROM rust:1.97.1-bookworm AS chef
RUN cargo install cargo-chef --locked --version "^0.1"
WORKDIR /app

# Capture the dependency graph as a recipe (independent of the source, so it is cached
# until the dependencies themselves change).
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build only the dependencies first — this layer is reused across source changes.
RUN cargo chef cook --release --locked --recipe-path recipe.json
# Then the workspace itself; only mosaic-server and its path deps compile here.
COPY . .
RUN cargo build --release --locked -p mosaic-server

# Minimal runtime: glibc from the same Debian release as the builder, a non-root user,
# and nothing else.
FROM debian:bookworm-slim AS runtime
RUN useradd --system --create-home --user-group mosaic
COPY --from=builder /app/target/release/mosaic-server /usr/local/bin/mosaic-server
USER mosaic
WORKDIR /home/mosaic
# Listen on all interfaces inside the container; publish with `-p` at run time. Liveness
# is GET /healthz (probe it from the orchestrator).
ENV MOSAIC_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["mosaic-server"]
