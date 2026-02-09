#!/usr/bin/env bash
set -euo pipefail

# Import zones from PowerDNS to MicroDNS
# Usage: ./import-zones.sh --target http://192.168.1.51:8080 --zones "gw.lo,apps.gw.lo"
# Requires: curl, python3

PDNS_API="http://dnsx.gw.lo:8081"
PDNS_API_KEY="X-API-Key: changeme"
TARGET=""
ZONES=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --target) TARGET="$2"; shift 2 ;;
        --zones) ZONES="${ZONES:+${ZONES},}$2"; shift 2 ;;
        --pdns-api) PDNS_API="$2"; shift 2 ;;
        --pdns-key) PDNS_API_KEY="X-API-Key: $2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [[ -z "$TARGET" || -z "$ZONES" ]]; then
    echo "Usage: $0 --target http://host:8080 --zones \"zone1,zone2\""
    exit 1
fi

MICRODNS_API="${TARGET}/api/v1"

IFS=',' read -ra ZONE_LIST <<< "$ZONES"

echo "==> PowerDNS API: $PDNS_API"
echo "==> MicroDNS API: $MICRODNS_API"
echo "==> Zones to import: ${ZONE_LIST[*]}"

for zone_name in "${ZONE_LIST[@]}"; do
    zone_name=$(echo "$zone_name" | xargs)  # trim whitespace
    # PowerDNS uses trailing dot for zone names
    pdns_zone="${zone_name}."

    echo ""
    echo "--- Importing zone: ${zone_name} ---"

    # Fetch zone from PowerDNS
    zone_data=$(curl -s -H "$PDNS_API_KEY" \
        "${PDNS_API}/api/v1/servers/localhost/zones/${pdns_zone}")

    if echo "$zone_data" | python3 -c "import sys,json; json.load(sys.stdin)" 2>/dev/null; then
        : # valid JSON
    else
        echo "  ERROR: Failed to fetch zone ${zone_name} from PowerDNS"
        continue
    fi

    # Extract SOA info
    soa_primary=$(echo "$zone_data" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for rrset in data.get('rrsets', []):
    if rrset['type'] == 'SOA':
        parts = rrset['records'][0]['content'].split()
        print(parts[0].rstrip('.'))
        break
else:
    print('ns1.${zone_name}')
" 2>/dev/null || echo "ns1.${zone_name}")

    soa_email=$(echo "$zone_data" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for rrset in data.get('rrsets', []):
    if rrset['type'] == 'SOA':
        parts = rrset['records'][0]['content'].split()
        print(parts[1].rstrip('.'))
        break
else:
    print('admin.${zone_name}')
" 2>/dev/null || echo "admin.${zone_name}")

    # Create zone in MicroDNS
    echo "  Creating zone ${zone_name}..."
    create_resp=$(curl -s -w "\n%{http_code}" -X POST "${MICRODNS_API}/zones" \
        -H "Content-Type: application/json" \
        -d "{
            \"name\": \"${zone_name}\",
            \"primary_ns\": \"${soa_primary}\",
            \"admin_email\": \"${soa_email}\",
            \"default_ttl\": 300
        }")

    http_code=$(echo "$create_resp" | tail -1)
    body=$(echo "$create_resp" | sed '$ d')

    if [[ "$http_code" == "201" || "$http_code" == "200" ]]; then
        zone_id=$(echo "$body" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
        echo "  Created zone ${zone_name} (id: ${zone_id})"
    elif [[ "$http_code" == "409" ]]; then
        echo "  Zone ${zone_name} already exists, fetching ID..."
        zones_resp=$(curl -s "${MICRODNS_API}/zones")
        zone_id=$(echo "$zones_resp" | python3 -c "
import sys, json
for z in json.load(sys.stdin):
    if z['name'] == '${zone_name}':
        print(z['id'])
        break
")
        echo "  Using existing zone ID: ${zone_id}"
    else
        echo "  ERROR: Failed to create zone ${zone_name} (HTTP ${http_code}): ${body}"
        continue
    fi

    # Import records
    record_count=0
    echo "$zone_data" | python3 -c "
import sys, json

data = json.load(sys.stdin)
zone_name = '${zone_name}'

for rrset in data.get('rrsets', []):
    rtype = rrset['type']

    # Skip SOA (auto-created) and NS at apex
    if rtype == 'SOA':
        continue

    name = rrset['name'].rstrip('.')
    # Convert FQDN to relative name
    if name == zone_name:
        name = '@'
    elif name.endswith('.' + zone_name):
        name = name[:-(len(zone_name) + 1)]

    ttl = rrset.get('ttl', 300)

    for record in rrset['records']:
        content = record['content']

        # Map record types
        if rtype == 'A':
            data_json = '{\"A\": \"' + content + '\"}'
        elif rtype == 'AAAA':
            data_json = '{\"AAAA\": \"' + content + '\"}'
        elif rtype == 'CNAME':
            data_json = '{\"CNAME\": \"' + content.rstrip('.') + '\"}'
        elif rtype == 'MX':
            parts = content.split()
            data_json = '{\"MX\": {\"preference\": ' + parts[0] + ', \"exchange\": \"' + parts[1].rstrip('.') + '\"}}'
        elif rtype == 'NS':
            data_json = '{\"NS\": \"' + content.rstrip('.') + '\"}'
        elif rtype == 'PTR':
            data_json = '{\"PTR\": \"' + content.rstrip('.') + '\"}'
        elif rtype == 'SRV':
            parts = content.split()
            data_json = '{\"SRV\": {\"priority\": ' + parts[0] + ', \"weight\": ' + parts[1] + ', \"port\": ' + parts[2] + ', \"target\": \"' + parts[3].rstrip('.') + '\"}}'
        elif rtype == 'TXT':
            # Strip surrounding quotes if present
            txt = content.strip('\"')
            data_json = '{\"TXT\": \"' + txt.replace('\"', '\\\\\"') + '\"}'
        else:
            continue

        print(json.dumps({
            'name': name,
            'ttl': ttl,
            'data': json.loads(data_json)
        }))
" | while IFS= read -r record_json; do
        resp=$(curl -s -w "\n%{http_code}" -X POST \
            "${MICRODNS_API}/zones/${zone_id}/records" \
            -H "Content-Type: application/json" \
            -d "$record_json")

        rc=$(echo "$resp" | tail -1)
        if [[ "$rc" == "201" || "$rc" == "200" ]]; then
            record_count=$((record_count + 1))
        else
            rname=$(echo "$record_json" | python3 -c "import sys,json; print(json.load(sys.stdin)['name'])" 2>/dev/null || echo "?")
            echo "  WARN: Failed to import record ${rname} (HTTP ${rc})"
        fi
    done

    echo "  Done importing ${zone_name}"
done

echo ""
echo "==> Import complete."
