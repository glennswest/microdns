#!/usr/bin/env bash
set -euo pipefail

# Build MicroDNS container image and push to local mkube registry.
# The registry syncs upstream to GHCR automatically.
#
# Usage:
#   ./scripts/build-and-push.sh              # build arm64, push to local registry as :edge
#   ./scripts/build-and-push.sh latest       # build arm64, push as :latest

REGISTRY="${REGISTRY:-192.168.200.2:5000}"
REPO="${REGISTRY}/microdns"
TAG="${1:-edge}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

echo "==> Building microdns for ARM64 (aarch64-unknown-linux-musl)..."
cargo build --release --target aarch64-unknown-linux-musl

echo "==> Building container image with podman..."
podman build --platform linux/arm64 \
    -t "${REPO}:${TAG}" \
    .

echo "==> Pushing to ${REPO}:${TAG}..."
podman push --tls-verify=false "${REPO}:${TAG}"

echo "==> Done. Image pushed to ${REPO}:${TAG}"
echo "    mkube image watcher will auto-deploy."
