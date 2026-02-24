#!/bin/bash
# Build, push, and deploy microdns to mkube on rose1
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

REGISTRY="192.168.200.2:5000"
MKUBE_API="http://192.168.200.2:8082"
IMAGE="$REGISTRY/microdns:edge"

echo "=== Deploying microdns ==="

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

# Push to local registry (mkube will detect and redeploy)
echo "Pushing to $REGISTRY..."
podman push --tls-verify=false "$IMAGE"

# Trigger mkube image redeploy
echo "Triggering image redeploy..."
curl -s -X POST "$MKUBE_API/api/v1/images/redeploy" || true

echo ""
echo "=== Done ==="
echo "Deployed microdns to $REGISTRY"
