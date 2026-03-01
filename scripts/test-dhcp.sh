#!/usr/bin/env bash
set -euo pipefail

# DHCP + DNS integration test for microdns on MikroTik/rose
# Tests all DNS zones and DHCP servers via their REST APIs
# Usage: ./scripts/test-dhcp.sh [dns-ip]

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

PASS=0
FAIL=0

pass() { printf "  ${GREEN}PASS${NC} %s\n" "$1"; PASS=$((PASS + 1)); }
fail() { printf "  ${RED}FAIL${NC} %s\n" "$1"; FAIL=$((FAIL + 1)); }
skip() { printf "  ${YELLOW}SKIP${NC} %s\n" "$1"; }

NETS="g10 gt g11"
SINGLE="${1:-}"

get_ip() {
    case "$1" in
        g10) echo "192.168.10.252" ;;
        gt)  echo "192.168.200.199" ;;
        g11) echo "192.168.11.252" ;;
    esac
}

get_zone() {
    case "$1" in
        g10) echo "g10.lo" ;;
        gt)  echo "gt.lo" ;;
        g11) echo "g11.lo" ;;
    esac
}

echo "========================================"
echo " MicroDNS DHCP + DNS Integration Tests"
echo "========================================"
echo ""

for net in $NETS; do
    ip=$(get_ip "$net")
    zone=$(get_zone "$net")

    if [ -n "$SINGLE" ] && [ "$ip" != "$SINGLE" ]; then
        continue
    fi

    echo "── $net ($ip) ─────────────────────────"

    # 1. Health check
    echo ""
    echo "  [Health]"
    if curl -sf --connect-timeout 3 "http://${ip}:8080/api/v1/health" 2>/dev/null | grep -q '"status":"ok"'; then
        pass "$net health endpoint"
    else
        fail "$net health endpoint (unreachable)"
        echo ""
        continue
    fi

    # 2. DNS SOA
    echo "  [DNS]"
    soa=$(dig @"$ip" "$zone" SOA +short +time=3 +tries=1 2>/dev/null)
    if [ -n "$soa" ]; then
        pass "$net SOA for $zone"
    else
        fail "$net SOA for $zone (no answer)"
    fi

    # 3. Zone list
    echo "  [Zones]"
    zones=$(curl -sf --connect-timeout 3 "http://${ip}:8080/api/v1/zones" 2>/dev/null) || zones=""
    if [ -n "$zones" ]; then
        zone_count=$(echo "$zones" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
        if [ "$zone_count" -gt 0 ]; then
            pass "$net has $zone_count zone(s)"
        else
            fail "$net has 0 zones"
        fi
    else
        fail "$net zone list unreachable"
    fi

    # 4. DHCP status
    echo "  [DHCP]"
    dhcp_status=$(curl -sf --connect-timeout 3 "http://${ip}:8080/api/v1/dhcp/status" 2>/dev/null) || dhcp_status=""
    if [ -z "$dhcp_status" ]; then
        skip "$net DHCP not configured"
    else
        enabled=$(echo "$dhcp_status" | python3 -c "import sys,json; print(json.load(sys.stdin).get('enabled', False))" 2>/dev/null)
        if [ "$enabled" = "True" ]; then
            pass "$net DHCP enabled (gateway/relay mode)"
            pool_count=$(echo "$dhcp_status" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('pools',[])))" 2>/dev/null || echo 0)
            res_count=$(echo "$dhcp_status" | python3 -c "import sys,json; print(json.load(sys.stdin).get('reservation_count',0))" 2>/dev/null || echo 0)
            lease_count=$(echo "$dhcp_status" | python3 -c "import sys,json; print(json.load(sys.stdin).get('active_lease_count',0))" 2>/dev/null || echo 0)
            pass "$net DHCP: $pool_count pool(s), $res_count reservation(s), $lease_count active lease(s)"

            pxe=$(echo "$dhcp_status" | python3 -c "import sys,json; d=json.load(sys.stdin); print(any(p.get('pxe_enabled') for p in d.get('pools',[])))" 2>/dev/null)
            if [ "$pxe" = "True" ]; then
                pass "$net PXE boot enabled"
            fi
        else
            skip "$net DHCP disabled (no DHCP pool on this network)"
        fi
    fi

    # 5. Active leases
    echo "  [Leases]"
    leases=$(curl -sf --connect-timeout 3 "http://${ip}:8080/api/v1/leases" 2>/dev/null) || leases="[]"
    echo "$leases" | python3 -c "
import sys, json
leases = json.load(sys.stdin)
for l in leases:
    print(f\"    {l['ip_addr']:18s} {l['mac_addr']:18s} {l.get('hostname','?'):15s} {l['state']}\")
if not leases:
    print('    (no active leases)')
" 2>/dev/null || true

    # 6. Cross-zone forwarding
    echo "  [Forwarding]"
    for other_net in $NETS; do
        if [ "$other_net" = "$net" ]; then continue; fi
        other_zone=$(get_zone "$other_net")
        result=$(dig @"$ip" "$other_zone" SOA +short +time=2 +tries=1 2>/dev/null)
        if [ -n "$result" ]; then
            pass "$net -> $other_zone forwarding"
        else
            fail "$net -> $other_zone forwarding"
        fi
    done

    echo ""
done

echo "========================================"
printf " Results: ${GREEN}%d passed${NC}, ${RED}%d failed${NC}\n" "$PASS" "$FAIL"
echo "========================================"

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
