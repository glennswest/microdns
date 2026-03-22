#!/usr/bin/env bash
set -euo pipefail

# Build MicroDNS container image and push to local mkube registry.
# The registry syncs upstream to GHCR automatically.
#
# Usage:
#   ./scripts/build-and-push.sh              # build arm64, push to local registry as :edge
#   ./scripts/build-and-push.sh latest       # build arm64, push as :latest

REGISTRY="${REGISTRY:-registry.gt.lo:5000}"
REPO="${REGISTRY}/microdns"
TAG="${1:-edge}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

echo "==> Building microdns for ARM64 (aarch64-unknown-linux-musl)..."
cargo build --release --target aarch64-unknown-linux-musl

echo "==> Preparing binary for scratch image..."
cp target/aarch64-unknown-linux-musl/release/microdns microdns

echo "==> Building container image with podman..."
podman build --tls-verify=false --platform linux/arm64 \
    -f Dockerfile.scratch \
    -t "${REPO}:${TAG}" \
    .

rm -f microdns

echo "==> Pushing to ${REPO}:${TAG}..."
podman push --tls-verify=false "${REPO}:${TAG}"

echo "==> Done. Image pushed to ${REPO}:${TAG}"
echo "    mkube image watcher will auto-deploy."
