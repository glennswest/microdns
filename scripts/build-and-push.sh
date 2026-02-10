#!/usr/bin/env bash
set -euo pipefail

# Build MicroDNS container image and push to GitHub Container Registry
# Usage:
#   ./scripts/build-and-push.sh              # build+push x86_64 as :latest
#   ./scripts/build-and-push.sh v1.0.0       # build+push x86_64 with tag
#   ./scripts/build-and-push.sh v1.0.0 multi # build+push multi-arch manifest

REPO="ghcr.io/glennswest/microdns"
TAG="${1:-latest}"
MODE="${2:-amd64}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Ensure logged in to ghcr.io
if ! docker login ghcr.io --get-login &>/dev/null 2>&1; then
    echo "Not logged in to ghcr.io. Run:"
    echo "  echo \$GITHUB_TOKEN | docker login ghcr.io -u glennswest --password-stdin"
    exit 1
fi

# Ensure buildx builder exists
if ! docker buildx inspect microdns-builder &>/dev/null 2>&1; then
    echo "==> Creating buildx builder..."
    docker buildx create --name microdns-builder --use
fi
docker buildx use microdns-builder

case "$MODE" in
    amd64)
        echo "==> Building x86_64 image and pushing to ${REPO}:${TAG}..."
        docker buildx build --platform linux/amd64 \
            -t "${REPO}:${TAG}" \
            -t "${REPO}:latest" \
            --push .
        ;;
    multi)
        echo "==> Building multi-arch manifest and pushing to ${REPO}:${TAG}..."
        docker buildx build --platform linux/amd64,linux/arm64 \
            -t "${REPO}:${TAG}" \
            -t "${REPO}:latest" \
            --push .
        ;;
    *)
        echo "Unknown mode: ${MODE}"
        echo "Usage: $0 [tag] [amd64|multi]"
        exit 1
        ;;
esac

echo "==> Pushed to ${REPO}:${TAG}"
echo "==> Done."
