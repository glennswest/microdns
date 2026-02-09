#!/usr/bin/env bash
set -euo pipefail

# Clone DHCP lease/reservation data from MikroTik routers and generate TOML fragments
# Usage: ./clone-dhcp-reservations.sh
# Requires: ssh access to admin@192.168.1.1 and admin@rose1.gw.lo

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUTPUT_DIR="$(dirname "$SCRIPT_DIR")/config/deploy"

mkdir -p "$OUTPUT_DIR"

parse_mikrotik_leases() {
    # Parse MikroTik lease output into TOML reservation format
    # Input: raw output from /ip dhcp-server lease print detail
    # Filters: only static bindings + dynamic leases with hostnames
    python3 -c "
import sys, re

text = sys.stdin.read()

# Split into entries
entries = re.split(r'\n\s*\d+\s+', '\n' + text)

for entry in entries:
    if not entry.strip():
        continue

    fields = {}
    for match in re.finditer(r'(\w[\w-]*)=([^\s]+|\"[^\"]*\")', entry):
        key = match.group(1)
        val = match.group(2).strip('\"')
        fields[key] = val

    mac = fields.get('mac-address', '').upper()
    ip = fields.get('address', '')
    hostname = fields.get('host-name', '')
    status = fields.get('status', '')
    dynamic = fields.get('dynamic', 'true')
    server = fields.get('server', '')
    comment = fields.get('comment', '')

    if not mac or not ip:
        continue

    # Include: all static bindings, plus dynamic leases that have hostnames
    if dynamic == 'false' or hostname:
        print('[[dhcp.v4.reservations]]')
        print(f'mac = \"{mac}\"')
        print(f'ip = \"{ip}\"')
        if hostname:
            print(f'hostname = \"{hostname}\"')
        elif comment:
            print(f'hostname = \"{comment}\"')
        if server:
            print(f'# server: {server}')
        print()
"
}

echo "==> Fetching leases from router.gw.lo (192.168.1.1)..."
ssh admin@192.168.1.1 '/ip dhcp-server lease print detail without-paging' 2>/dev/null | \
    parse_mikrotik_leases > "${OUTPUT_DIR}/reservations-main-raw.toml"

echo "==> Fetching leases from rose1.gw.lo..."
ssh admin@rose1.gw.lo '/ip dhcp-server lease print detail without-paging' 2>/dev/null | \
    parse_mikrotik_leases > "${OUTPUT_DIR}/reservations-rose1-raw.toml"

# Split rose1 leases by server (dhcp10 vs dhcp11)
echo "==> Splitting rose1 leases by DHCP server..."

python3 -c "
import re

with open('${OUTPUT_DIR}/reservations-rose1-raw.toml') as f:
    text = f.read()

# Split into blocks
blocks = text.strip().split('\n\n')

g10_entries = []
g11_entries = []

for block in blocks:
    if not block.strip():
        continue
    if 'dhcp10' in block or '192.168.10.' in block:
        # Remove server comment line for clean output
        lines = [l for l in block.split('\n') if not l.startswith('# server:')]
        g10_entries.append('\n'.join(lines))
    elif 'dhcp11' in block or '192.168.11.' in block:
        lines = [l for l in block.split('\n') if not l.startswith('# server:')]
        g11_entries.append('\n'.join(lines))

with open('${OUTPUT_DIR}/reservations-g10.toml', 'w') as f:
    f.write('# DHCP reservations for g10 (192.168.10.x)\n')
    f.write('# Auto-generated from rose1.gw.lo DHCP leases\n\n')
    f.write('\n\n'.join(g10_entries))
    f.write('\n')

with open('${OUTPUT_DIR}/reservations-g11.toml', 'w') as f:
    f.write('# DHCP reservations for g11 (192.168.11.x)\n')
    f.write('# Auto-generated from rose1.gw.lo DHCP leases\n\n')
    f.write('\n\n'.join(g11_entries))
    f.write('\n')
"

# Clean up main reservations
python3 -c "
with open('${OUTPUT_DIR}/reservations-main-raw.toml') as f:
    text = f.read()

blocks = text.strip().split('\n\n')
clean = []
for block in blocks:
    if not block.strip():
        continue
    lines = [l for l in block.split('\n') if not l.startswith('# server:')]
    clean.append('\n'.join(lines))

with open('${OUTPUT_DIR}/reservations-main.toml', 'w') as f:
    f.write('# DHCP reservations for main (192.168.1.x)\n')
    f.write('# Auto-generated from router.gw.lo DHCP leases\n\n')
    f.write('\n\n'.join(clean))
    f.write('\n')
"

# Cleanup raw files
rm -f "${OUTPUT_DIR}/reservations-main-raw.toml" "${OUTPUT_DIR}/reservations-rose1-raw.toml"

echo ""
echo "==> Generated reservation files:"
echo "  ${OUTPUT_DIR}/reservations-main.toml"
echo "  ${OUTPUT_DIR}/reservations-g10.toml"
echo "  ${OUTPUT_DIR}/reservations-g11.toml"
echo ""
echo "Copy the [[dhcp.v4.reservations]] entries into the respective config files."
