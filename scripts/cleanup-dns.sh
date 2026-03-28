#!/bin/bash
# cleanup-dns.sh — Remove stale DNS records and leases from all microdns instances
# Deletes A/PTR records that have no matching active lease or reservation
# Preserves: reservations, infrastructure records (dns, microdns.dns, rose1, pxe, switch*, ipmiserial, bmh-operator)
# Only cleans DHCP-enabled instances — non-DHCP networks (gt, gw) are skipped since their
# DNS records are managed by mkube container auto-registration, not DHCP.

set -euo pipefail

# Only include instances that run DHCP — their DNS records are DHCP-backed
# gt and gw do NOT run DHCP; their records come from mkube container registration
INSTANCES=(
    "g10|192.168.10.252"
    "g11|192.168.11.252"
)

MAX_AGE_MINUTES="${1:-60}"
NOW=$(date +%s)

log() { echo "[$(date '+%H:%M:%S')] $*"; }

api() {
    local ip="$1" method="$2" path="$3"
    shift 3
    curl -sf -X "$method" "http://${ip}:8080/api/v1${path}" \
        -H 'Content-Type: application/json' "$@" 2>/dev/null
}

# Infrastructure hostnames to never delete (case-insensitive match)
is_infra() {
    local name="$1"
    case "$name" in
        dns|ns1|ns2|microdns.*|rose1|pxe|switch*|ipmiserial*|bmh-operator*) return 0 ;;
        *) return 1 ;;
    esac
}

iso_to_epoch() {
    # Parse ISO 8601 timestamp to epoch seconds
    local ts="$1"
    # Strip fractional seconds and timezone for portable parsing
    local clean=$(echo "$ts" | sed -E 's/\.[0-9]+//; s/\+00:00$/Z/; s/Z$//')
    date -j -f "%Y-%m-%dT%H:%M:%S" "$clean" +%s 2>/dev/null || echo 0
}

for entry in "${INSTANCES[@]}"; do
    IFS='|' read -r name ip <<< "$entry"
    log "=== Instance: $name ($ip) ==="

    # Check reachability
    if ! curl -sf "http://${ip}:8080/api/v1/health" >/dev/null 2>&1; then
        log "  SKIP — unreachable"
        continue
    fi

    # Gather current state
    reservations=$(api "$ip" GET /dhcp/reservations || echo "[]")
    leases=$(api "$ip" GET /leases || echo "[]")
    zones=$(api "$ip" GET /zones || echo "[]")

    # Build sets of reserved IPs, reserved hostnames, and active lease IPs
    reserved_ips=$(echo "$reservations" | python3 -c "import sys,json; [print(r['ip']) for r in json.load(sys.stdin)]" 2>/dev/null || true)
    reserved_hosts=$(echo "$reservations" | python3 -c "import sys,json; [print(r.get('hostname','')) for r in json.load(sys.stdin) if r.get('hostname')]" 2>/dev/null || true)
    active_lease_ips=$(echo "$leases" | python3 -c "import sys,json; [print(l['ip_addr']) for l in json.load(sys.stdin)]" 2>/dev/null || true)

    for zone_row in $(echo "$zones" | python3 -c "import sys,json; [print(z['id']+'|'+z['name']) for z in json.load(sys.stdin)]" 2>/dev/null); do
        IFS='|' read -r zone_id zone_name <<< "$zone_row"
        log "  Zone: $zone_name ($zone_id)"

        records=$(api "$ip" GET "/zones/${zone_id}/records?limit=500" || echo "[]")

        echo "$records" | python3 -c "
import sys, json
records = json.load(sys.stdin)
for r in records:
    rid = r['id']
    rname = r['name']
    rtype = r['type']
    rdata = r['data'].get('data','') if isinstance(r['data'], dict) else ''
    created = r.get('created_at','')
    print(f'{rid}|{rname}|{rtype}|{rdata}|{created}')
" 2>/dev/null | while IFS='|' read -r rec_id rec_name rec_type rec_data rec_created; do

            # Skip infra records
            if is_infra "$rec_name"; then
                continue
            fi

            should_delete=false
            reason=""

            if [[ "$rec_type" == "A" ]]; then
                rec_ip="$rec_data"
                # Check if this IP belongs to a reservation or active lease
                if echo "$reserved_ips" | grep -qx "$rec_ip"; then
                    # IP is reserved — check hostname matches
                    if echo "$reserved_hosts" | grep -qx "$rec_name"; then
                        continue  # valid reservation record
                    fi
                    # A record IP matches a reservation but hostname doesn't — could be stale DHCP auto-reg
                fi
                if echo "$active_lease_ips" | grep -qx "$rec_ip"; then
                    continue  # active lease, keep it
                fi
                # Not reserved and not actively leased — check age
                if [[ -n "$rec_created" ]]; then
                    rec_epoch=$(iso_to_epoch "$rec_created")
                    age_minutes=$(( (NOW - rec_epoch) / 60 ))
                    if [[ $age_minutes -gt $MAX_AGE_MINUTES ]]; then
                        should_delete=true
                        reason="A record $rec_name -> $rec_ip (age: ${age_minutes}m, no reservation/lease)"
                    fi
                fi

            elif [[ "$rec_type" == "PTR" ]]; then
                # For PTR, rec_name is the last octet, rec_data is the FQDN
                ptr_hostname=$(echo "$rec_data" | sed 's/\..*//')  # extract hostname from FQDN

                if is_infra "$ptr_hostname"; then
                    continue
                fi

                # Reconstruct the IP from the zone name and record name
                # Reverse zone name like "10.168.192.in-addr.arpa" -> prefix "192.168.10"
                prefix=$(echo "$zone_name" | sed 's/\.in-addr\.arpa$//' | awk -F. '{for(i=NF;i>0;i--) printf "%s%s",$i,(i>1?".":"")}')
                ptr_ip="${prefix}.${rec_name}"

                if echo "$reserved_ips" | grep -qx "$ptr_ip"; then
                    # Check the PTR hostname matches the reservation hostname for this IP
                    expected_host=$(echo "$reservations" | python3 -c "
import sys,json
for r in json.load(sys.stdin):
    if r['ip'] == '$ptr_ip':
        print(r.get('hostname',''))
        break
" 2>/dev/null || true)
                    if [[ "$ptr_hostname" == "$expected_host" ]]; then
                        continue  # correct PTR
                    fi
                    # PTR points to wrong hostname for this reserved IP — stale
                    should_delete=true
                    reason="PTR $rec_name -> $rec_data (IP $ptr_ip reserved for '$expected_host', not '$ptr_hostname')"
                elif echo "$active_lease_ips" | grep -qx "$ptr_ip"; then
                    continue  # active lease, keep it
                else
                    # Not reserved, not leased — check age
                    if [[ -n "$rec_created" ]]; then
                        rec_epoch=$(iso_to_epoch "$rec_created")
                        age_minutes=$(( (NOW - rec_epoch) / 60 ))
                        if [[ $age_minutes -gt $MAX_AGE_MINUTES ]]; then
                            should_delete=true
                            reason="PTR $rec_name -> $rec_data (IP $ptr_ip, age: ${age_minutes}m, no reservation/lease)"
                        fi
                    fi
                fi
            fi

            if [[ "$should_delete" == "true" ]]; then
                log "    DELETE: $reason"
                http_code=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE \
                    "http://${ip}:8080/api/v1/zones/${zone_id}/records/${rec_id}" 2>/dev/null || echo "ERR")
                if [[ "$http_code" == "204" ]]; then
                    log "      -> deleted"
                else
                    log "      -> FAILED (HTTP $http_code)"
                fi
            fi
        done
    done
done

log "=== Cleanup complete ==="
