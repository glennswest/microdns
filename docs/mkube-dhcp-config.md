# MicroDNS DHCP Pool Configuration for mkube

## Problem

When mkube creates/updates DHCP pools via the microdns REST API, the `domain_search` field needs to include all `.lo` zones so that systemd-resolved on clients can resolve names across networks (e.g. a g10 client resolving `registry.gt.lo`).

Without this, DHCP option 119 (domain search list) only contains the local zone, and cross-network DNS lookups fail.

## Required Pool Fields

When mkube pushes a DHCP pool via `POST /api/v1/dhcp/pools` or `PATCH /api/v1/dhcp/pools/{id}`, include:

```json
{
  "domain_search": ["<local>.lo", "gt.lo", "gw.lo", "g10.lo", "g11.lo", "g8.lo", "g9.lo", "pv.lo", "bm.lo", "ipmi.lo"]
}
```

The local zone should be first in the list (it's used for hostname completion). The remaining zones are routing domains that tell systemd-resolved to send queries for those zones to the local microdns instance.

## Per-Network domain_search Values

| Network | domain_search |
|---------|--------------|
| g8  | `["g8.lo", "gt.lo", "gw.lo", "g10.lo", "g11.lo", "g9.lo", "pv.lo", "bm.lo", "ipmi.lo"]` |
| g9  | `["g9.lo", "gt.lo", "gw.lo", "g10.lo", "g11.lo", "g8.lo", "pv.lo", "bm.lo", "ipmi.lo"]` |
| g10 | `["g10.lo", "gt.lo", "gw.lo", "g11.lo", "g8.lo", "g9.lo", "pv.lo", "bm.lo", "ipmi.lo"]` |
| g11 | `["g11.lo", "gt.lo", "gw.lo", "g10.lo", "g8.lo", "g9.lo", "pv.lo", "bm.lo", "ipmi.lo"]` |
| gt  | `["gt.lo", "gw.lo", "g10.lo", "g11.lo", "g8.lo", "g9.lo", "pv.lo", "bm.lo", "ipmi.lo"]` |
| gw  | `["gw.lo", "gt.lo", "g10.lo", "g11.lo", "g8.lo", "g9.lo", "pv.lo", "bm.lo", "ipmi.lo"]` |

## Why This Matters

DHCP option 15 (domain name) causes systemd-resolved to scope the DNS server to that single domain. A client on g10 with `DNS Domain: g10.lo` will only query `192.168.10.252` for `*.g10.lo` names — lookups for `registry.gt.lo` go to a fallback resolver (or fail).

MicroDNS now suppresses option 15 when `domain_search` is set. Option 119 (domain search) provides both hostname completion and routing domains without the scoping side-effect. Each zone listed in `domain_search` becomes a routing domain in systemd-resolved, so queries for any `.lo` zone go through the local microdns instance which forwards to the correct peer.

## REST API Examples

### Create pool with domain_search
```bash
curl -X POST http://192.168.10.252:8080/api/v1/dhcp/pools \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "g10-pool",
    "range_start": "192.168.10.100",
    "range_end": "192.168.10.199",
    "subnet": "192.168.10.0/24",
    "gateway": "192.168.10.1",
    "dns_servers": ["192.168.10.252"],
    "domain": "g10.lo",
    "lease_time_secs": 600,
    "domain_search": ["g10.lo", "gt.lo", "gw.lo", "g11.lo", "g8.lo", "g9.lo", "pv.lo", "bm.lo", "ipmi.lo"]
  }'
```

### Update existing pool
```bash
curl -X PATCH http://192.168.10.252:8080/api/v1/dhcp/pools/{id} \
  -H 'Content-Type: application/json' \
  -d '{
    "domain_search": ["g10.lo", "gt.lo", "gw.lo", "g11.lo", "g8.lo", "g9.lo", "pv.lo", "bm.lo", "ipmi.lo"]
  }'
```

## Option 15 Behavior

- When `domain_search` is set and non-empty: option 15 (domain name) is **not sent**
- When `domain_search` is empty or absent: option 15 is sent using the `domain` field as before

The `domain` field can still be set on the pool for backward compatibility — it just won't be emitted as option 15 when domain_search is active.
