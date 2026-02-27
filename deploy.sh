#!/bin/bash
# Build, push, and deploy microdns to mkube on rose1
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

REGISTRY="registry.gt.lo:5000"
MKUBE_API="http://192.168.200.2:8082"
IMAGE="$REGISTRY/microdns:edge"

echo "=== Deploying microdns ==="

# Build
"$SCRIPT_DIR/build.sh"

# Push to local registry
echo "Pushing to $REGISTRY..."
podman push --tls-verify=false "$IMAGE"

# Trigger mkube to pick up the new image immediately
echo "Triggering image redeploy..."
curl -s -X POST "$MKUBE_API/api/v1/images/redeploy" || true

echo ""
echo "=== Done ==="
echo "Deployed microdns to $REGISTRY"
echo "DNS pods auto-updated by mkube"
