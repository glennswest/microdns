#!/usr/bin/env bash
set -euo pipefail

# Build MicroDNS ARM64 binary and create RouterOS-compatible tarball
# Requires: docker with buildx

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
IMAGE_NAME="microdns:arm64"
OUTPUT_DIR="${PROJECT_DIR}"

cd "$PROJECT_DIR"

echo "==> Building ARM64 image..."
docker build -t "$IMAGE_NAME" .

echo "==> Extracting binary from image..."
docker create --name mdns-extract "$IMAGE_NAME" 2>/dev/null || {
    docker rm mdns-extract
    docker create --name mdns-extract "$IMAGE_NAME"
}
docker cp mdns-extract:/microdns "${OUTPUT_DIR}/microdns-arm64"
docker rm mdns-extract

echo "==> Creating RouterOS container tarball..."
# RouterOS expects a tarball with the rootfs
TARBALL_DIR=$(mktemp -d)
mkdir -p "${TARBALL_DIR}/etc/microdns"
mkdir -p "${TARBALL_DIR}/data"
cp "${OUTPUT_DIR}/microdns-arm64" "${TARBALL_DIR}/microdns"
chmod +x "${TARBALL_DIR}/microdns"

# Create a minimal /etc/passwd and /etc/group for scratch-like container
echo "root:x:0:0:root:/:/microdns" > "${TARBALL_DIR}/etc/passwd"
echo "root:x:0:" > "${TARBALL_DIR}/etc/group"

# Tarball
tar -C "${TARBALL_DIR}" -czf "${OUTPUT_DIR}/microdns-arm64.tar.gz" .
rm -rf "${TARBALL_DIR}"

echo "==> Output:"
echo "  Binary:  ${OUTPUT_DIR}/microdns-arm64"
echo "  Tarball: ${OUTPUT_DIR}/microdns-arm64.tar.gz"
echo "==> Done."
