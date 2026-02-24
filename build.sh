#!/bin/bash
# Build microdns binary and container image locally
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

IMAGE="microdns:latest"

echo "=== Building microdns ==="

# Cross-compile binary locally for ARM64 Linux (static musl)
echo "Building binary for aarch64-unknown-linux-musl..."
cargo build --release --target aarch64-unknown-linux-musl

# Copy binary to project root for Dockerfile
cp target/aarch64-unknown-linux-musl/release/microdns microdns

# Build scratch container image with podman
echo "Building container image..."
podman build --platform linux/arm64 -f Dockerfile.scratch -t "$IMAGE" .

# Clean up local binary copy
rm -f microdns

echo ""
echo "=== Build complete ==="
echo "Image: $IMAGE"
echo "Run ./deploy.sh to push and deploy to rose1"
