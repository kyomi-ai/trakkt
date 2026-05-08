#!/usr/bin/env bash
# Build trakkt Docker image from host-compiled artifacts.
#
# Usage:
#   ./scripts/build-image.sh [image-tag]
#
# Default tag: trakkt:latest

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

IMAGE_TAG="${1:-trakkt:latest}"

echo "==> Building trakkt Docker image: $IMAGE_TAG"
echo ""

# --- 1. Strip local dev patches from Cargo.toml ---
CARGO_TOML="$REPO_ROOT/Cargo.toml"
CARGO_TOML_BAK="$REPO_ROOT/Cargo.toml.bak"

cp "$CARGO_TOML" "$CARGO_TOML_BAK"
cleanup() {
    mv "$CARGO_TOML_BAK" "$CARGO_TOML"
}
trap cleanup EXIT

sed -i '/# BEGIN_LOCAL_DEV_PATCHES/,/# END_LOCAL_DEV_PATCHES/{ /# BEGIN_LOCAL_DEV_PATCHES/!{ /# END_LOCAL_DEV_PATCHES/!d } }' "$CARGO_TOML"

echo "==> [1/3] Building server binary (release)..."
cargo build --release --bin trakkt

echo ""
echo "==> [2/3] Building Leptos frontend (release)..."
cd "$REPO_ROOT/crates/trakkt-ui"
trunk build --release
cd "$REPO_ROOT"

echo ""
echo "==> [3/3] Building Docker image..."
docker build -t "$IMAGE_TAG" .

echo ""
echo "✅ Done! Image built: $IMAGE_TAG"
echo ""
echo "To push to a registry:"
echo "  docker tag $IMAGE_TAG your-registry/$IMAGE_TAG"
echo "  docker push your-registry/$IMAGE_TAG"
echo ""
echo "To deploy to Kubernetes:"
echo "  cd deploy/k8s && ./deploy.sh"
