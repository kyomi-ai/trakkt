# ---------------------------------------------------------------------------
# Trakkt — packaging-only Docker image
# ---------------------------------------------------------------------------
#
# Prerequisites: build artifacts must be produced on the HOST before running
# docker build. Use the build script:
#
#   ./scripts/build-image.sh
#
# That script:
#   1. Strips BEGIN_LOCAL_DEV_PATCHES from Cargo.toml
#   2. Runs: cargo build --profile dev-server --bin trakkt
#   3. Runs: cd crates/trakkt-ui && trunk build --release
#   4. Calls: docker build -t trakkt:latest .
#
# IMPORTANT: Binary and WASM frontend must be compiled on the same host so
# that CARGO_MANIFEST_DIR is identical for both. Leptos server_fn hashes
# include this path — mismatched builds break the app.
# ---------------------------------------------------------------------------

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/sh trakkt

WORKDIR /app

# Copy pre-built server binary (built with dev-server or release profile)
# dev-server: cargo build --profile dev-server --bin trakkt
# release:    cargo build --release --bin trakkt
ARG PROFILE=dev-server
COPY target/${PROFILE}/trakkt /app/trakkt

# Copy pre-built Leptos frontend assets
COPY crates/trakkt-ui/dist /app/dist

ENV PORT=8003 \
    TRUNK_DIST_DIR=/app/dist \
    RUST_LOG=info \
    HOME=/tmp \
    TMPDIR=/tmp

EXPOSE 8003

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/bin/sh", "-c", "wget -qO- http://localhost:8003/health || exit 1"]

USER trakkt

ENTRYPOINT ["/app/trakkt"]
