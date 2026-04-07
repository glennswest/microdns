#!/bin/bash
set -euo pipefail

TARGET_HOST="192.168.1.252"
TARGET_BIN="/usr/local/bin/microdns"
BINARY="target/x86_64-unknown-linux-musl/release/microdns"

if [ ! -f "$BINARY" ]; then
    echo "Binary not found: $BINARY"
    echo "Run: PATH=/opt/homebrew/opt/musl-cross/bin:\$PATH CC_x86_64_unknown_linux_musl=x86_64-linux-musl-gcc cargo build --release --target x86_64-unknown-linux-musl"
    exit 1
fi

STRIP="/opt/homebrew/opt/musl-cross/bin/x86_64-linux-musl-strip"
echo "Stripping binary..."
$STRIP "$BINARY"

SIZE=$(du -h "$BINARY" | cut -f1)
echo "Deploying microdns ($SIZE) to $TARGET_HOST..."

echo "Stopping microdns on $TARGET_HOST..."
ssh root@$TARGET_HOST 'rc-service microdns stop' || true

echo "Uploading binary..."
scp "$BINARY" root@$TARGET_HOST:$TARGET_BIN

echo "Setting permissions..."
ssh root@$TARGET_HOST "chmod +x $TARGET_BIN"

echo "Starting microdns on $TARGET_HOST..."
ssh root@$TARGET_HOST 'rc-service microdns start'

echo "Verifying..."
sleep 1
ssh root@$TARGET_HOST 'rc-service microdns status'

echo "Done."
