#!/bin/sh
# MicroDNS post-install: ensure state dir exists and refresh systemd.
set -e

mkdir -p /var/lib/microdns

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
fi

echo "MicroDNS installed."
echo "  1. Edit /etc/microdns/microdns.toml"
echo "  2. systemctl enable --now microdns"
