# ---------------------------------------------------------------------------
# Trakkt — unified Docker image for k8s and self-hosted deployments
# ---------------------------------------------------------------------------
#
# Mode is determined at runtime by environment variables (DATABASE_URL,
# REDIS_URL, TRAKKT_MODE, etc.) — the compiled binary is identical.
#
# Build prerequisites: Rust stable toolchain with trunk for Leptos builds.
# Builds in one stage: frontend (Leptos/WASM) + server binary.
#
# Build context must be the repo root:
#   docker build -t trakkt .
#
# Build arguments:
#   CARGO_PROFILE: cargo profile to use (default: release)
#
# Self-hosted quickstart:
#   docker run -e DATABASE_URL=sqlite:///data/trakkt.db -e TRAKKT_MODE=personal -p 8003:8003 trakkt
#
# Kubernetes: use deploy/k8s/ manifests with ConfigMap + Secrets for env vars.
# ---------------------------------------------------------------------------

FROM rust:latest as builder

RUN apt-get update && apt-get install -y \
    brotli \
    npm \
    && rm -rf /var/lib/apt/lists/*

# Install trunk for Leptos builds
RUN cargo install trunk

# Install Tailwind CSS CLI
RUN npm install -g tailwindcss

WORKDIR /build

# Copy workspace and source
COPY . .

# Build frontend with Leptos (release mode by default)
WORKDIR /build/crates/trakkt-ui
RUN trunk build --release

# Build server binary (release mode)
WORKDIR /build
RUN cargo build --release --bin trakkt

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/trakkt /app/trakkt

# Copy frontend assets to expected location
COPY --from=builder /build/crates/trakkt-ui/dist /app/dist

# Non-root user
RUN useradd -m -u 1000 trakkt

ENV PORT=8003 \
    TRUNK_DIST_DIR=/app/dist \
    RUST_LOG=info \
    HOME=/tmp \
    TMPDIR=/tmp

EXPOSE 8003

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD /app/trakkt health || exit 1

USER trakkt

ENTRYPOINT ["/app/trakkt"]
