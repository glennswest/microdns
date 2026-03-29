#!/bin/bash
# Bootstrap gw252 — transfer zones from .52, create reverse zone,
# pre-populate A+PTR for DHCP reservations, clean up junk records.
# Run AFTER .252 instance is deployed and responding.

set -euo pipefail

API="http://192.168.1.252:8080/api/v1"
OLD="192.168.1.52:53"

echo "=== Waiting for .252 to be ready ==="
for i in $(seq 1 30); do
    if curl -sf "$API/health" >/dev/null 2>&1; then
        echo "  .252 is up"
        break
    fi
    echo "  attempt $i/30..."
    sleep 2
done
curl -sf "$API/health" >/dev/null || { echo "FATAL: .252 not reachable"; exit 1; }

# ── Step 1: Transfer forward zones from .52 ──
echo ""
echo "=== Transferring zones from .52 ==="
for zone in gw.lo apps.gw.lo ai.gw.lo; do
    echo "  Transferring $zone..."
    curl -sf -X POST "$API/zones/transfer" \
        -H 'Content-Type: application/json' \
        -d "{\"zone\": \"$zone\", \"primary\": \"$OLD\"}" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(f"    -> {d.get(\"name\",\"?\")} ({d.get(\"record_count\",\"?\")} records)")'
done

# ── Step 2: Create empty reverse zone ──
echo ""
echo "=== Creating reverse zone ==="
curl -sf -X POST "$API/zones" \
    -H 'Content-Type: application/json' \
    -d '{"name": "1.168.192.in-addr.arpa"}' | python3 -c 'import json,sys; d=json.load(sys.stdin); print(f"  -> {d.get(\"name\",\"?\")} created")'

# ── Step 3: Get zone IDs ──
echo ""
echo "=== Resolving zone IDs ==="
ZONES=$(curl -sf "$API/zones")
GW_ZONE=$(echo "$ZONES" | python3 -c 'import json,sys; z=[x for x in json.load(sys.stdin) if x["name"]=="gw.lo"]; print(z[0]["id"] if z else "")')
REV_ZONE=$(echo "$ZONES" | python3 -c 'import json,sys; z=[x for x in json.load(sys.stdin) if x["name"]=="1.168.192.in-addr.arpa"]; print(z[0]["id"] if z else "")')
echo "  gw.lo     = $GW_ZONE"
echo "  reverse   = $REV_ZONE"

[ -z "$GW_ZONE" ] && { echo "FATAL: gw.lo zone not found"; exit 1; }
[ -z "$REV_ZONE" ] && { echo "FATAL: reverse zone not found"; exit 1; }

# Helper: create A record
create_a() {
    local name="$1" ip="$2"
    curl -sf -X POST "$API/zones/$GW_ZONE/records" \
        -H 'Content-Type: application/json' \
        -d "{\"name\": \"$name\", \"ttl\": 300, \"data\": {\"type\": \"A\", \"data\": \"$ip\"}, \"enabled\": true}" >/dev/null 2>&1 || true
}

# Helper: create PTR record
create_ptr() {
    local octet="$1" hostname="$2"
    curl -sf -X POST "$API/zones/$REV_ZONE/records" \
        -H 'Content-Type: application/json' \
        -d "{\"name\": \"$octet\", \"ttl\": 300, \"data\": {\"type\": \"PTR\", \"data\": \"${hostname}.gw.lo\"}, \"enabled\": true}" >/dev/null 2>&1 || true
}

# ── Step 4: Pre-create A + PTR for all DHCP reservations ──
echo ""
echo "=== Pre-creating A + PTR records for DHCP reservations ==="

# hostname  IP  last-octet
declare -a RESERVATIONS=(
    "monitor         192.168.1.153   153"
    "bootstrap       192.168.1.200   200"
    "network         192.168.1.201   201"
    "control1        192.168.1.202   202"
    "control2        192.168.1.203   203"
    "worker0         192.168.1.204   204"
    "worker1         192.168.1.205   205"
    "worker2         192.168.1.206   206"
    "dev             192.168.1.151   151"
    "workmac         192.168.1.18    18"
    "cap01           192.168.1.254   254"
    "generator       192.168.1.32    32"
    "super           192.168.1.67    67"
    "nvr             192.168.1.40    40"
    "epson           192.168.1.33    33"
    "frame1          192.168.1.30    30"
    "bay2            192.168.1.76    76"
    "washerdryer     192.168.1.78    78"
    "espx            192.168.1.79    79"
    "graylog         192.168.1.87    87"
    "minio           192.168.1.55    55"
    "pbs             192.168.1.165   165"
    "storex          192.168.1.161   161"
    "traefik         192.168.1.168   168"
    "standb          192.168.1.72    72"
    "logs            192.168.1.92    92"
    "boot            192.168.1.5     5"
    "rhel9full       192.168.1.175   175"
    "sv08            192.168.1.106   106"
    "hub             192.168.1.115   115"
    "vweb            192.168.1.116   116"
    "bay1            192.168.1.118   118"
    "ecoflow         192.168.1.17    17"
    "registry        192.168.1.80    80"
    "ting            192.168.1.9     9"
    "cap02           192.168.1.253   253"
    "rose1           192.168.1.88    88"
    "naman           192.168.1.135   135"
    "cp0             192.168.1.210   210"
    "cp1             192.168.1.211   211"
    "cp2             192.168.1.212   212"
    "node0           192.168.1.213   213"
    "node1           192.168.1.214   214"
    "node2           192.168.1.215   215"
    "node3           192.168.1.218   218"
    "node            192.168.1.127   127"
    "ap01            192.168.1.95    95"
)

for entry in "${RESERVATIONS[@]}"; do
    read -r name ip octet <<< "$entry"
    echo "  $name -> $ip (PTR $octet)"
    create_a "$name" "$ip"
    create_ptr "$octet" "$name"
done

# ── Step 5: Update dns record from .52 to .252 ──
echo ""
echo "=== Updating dns record to .252 ==="
# Find and delete old dns→.52, create dns→.252
DNS_REC=$(curl -sf "$API/zones/$GW_ZONE/records?limit=200" | python3 -c '
import json,sys
for r in json.load(sys.stdin):
    if r["name"] == "dns" and r["type"] == "A" and r["data"]["data"] == "192.168.1.52":
        print(r["id"]); break
')
if [ -n "$DNS_REC" ]; then
    curl -sf -X DELETE "$API/zones/$GW_ZONE/records/$DNS_REC" >/dev/null
    echo "  Deleted old dns -> .52"
fi
create_a "dns" "192.168.1.252"
echo "  Created dns -> .252"

# ── Step 6: Delete junk records ──
echo ""
echo "=== Cleaning up junk records ==="

# Build delete list: name patterns to remove
JUNK_NAMES=(
    "Glenns-iPad"
    "iPhone"
    "iPad"
    "Trang-Cutest"
    "trangs-iPhone"
    "trangsiphone"
    "Mac"
    "MacBookPro"
    "kindle2"
    "Blink DHCP"
    "Blink-Device"
    "Tesla_Model_3"
    "teslamodel3"
    "teslamodely"
    "tesla"
    "tesla1"
    "tesla2"
    "tesla3"
    "tesla4"
    "tesla5"
    "tesla6"
    "tesla7"
    "tesla8"
    "tesla9"
    "tesla10"
    "EPSON15B9E5"
    "GEMODULE25B6"
    "Aqara-Hub-M200-1DD8"
    "adt_S40LR0_01"
    "Ting-EA-33"
    "Gsw14"
    "Samsung"
    "konnected-b8959c"
    "konnected-f0f5bd524348"
    "cap01.g9.lo"
    "cap02.g9.lo"
    "cap03"
    "gw17"
    "gwest-mac"
    "h2c-one"
    "h2c-two"
    "worker-0"
    "worker-1"
    "worker-2"
    "winnode-0"
    "homekit"
    "mdns"
)

# Get all record IDs to delete
ALL_RECS=$(curl -sf "$API/zones/$GW_ZONE/records?limit=200")
for junk in "${JUNK_NAMES[@]}"; do
    IDS=$(echo "$ALL_RECS" | python3 -c "
import json,sys
for r in json.load(sys.stdin):
    if r['name'] == '$junk':
        print(r['id'])
")
    for id in $IDS; do
        curl -sf -X DELETE "$API/zones/$GW_ZONE/records/$id" >/dev/null 2>&1 || true
        echo "  Deleted: $junk"
    done
done

# ── Step 7: Summary ──
echo ""
echo "=== Done ==="
FINAL_ZONES=$(curl -sf "$API/zones")
echo "$FINAL_ZONES" | python3 -c '
import json,sys
for z in sorted(json.load(sys.stdin), key=lambda x: x["name"]):
    print(f"  {z[\"name\"]:35s} {z[\"record_count\"]} records")
'
echo ""
echo "Verify: dig @192.168.1.252 pvex.gw.lo A"
echo "Verify: dig @192.168.1.252 -x 192.168.1.160"
echo "Verify: dig @192.168.1.252 server1.g10.lo A"
