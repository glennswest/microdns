# MicroDNS DHCP Pool Configuration for mkube

## Overview

mkube creates and manages DHCP pools via the microdns REST API. Each managed network gets a pool with gateway, DNS, domain, and domain search list configured automatically from the Network CRD.

## How mkube Seeds DHCP Pools

When mkube boots or a new managed network is created, `seedDHCPPool()` creates or updates the pool on the target microdns instance. If a pool with matching subnet already exists, it updates it (via PATCH) to ensure `dns_servers` and `domain_search` are set.

The pool is built from the Network CRD spec:

| Pool field | Source |
|-----------|--------|
| `range_start` | `spec.dhcp.rangeStart` |
| `range_end` | `spec.dhcp.rangeEnd` |
| `subnet` | `spec.cidr` |
| `gateway` | `spec.gateway` |
| `dns_servers` | Target network's `spec.dns.server` |
| `domain` | `spec.dns.zone` |
| `domain_search` | `[spec.dns.zone]` (local zone only) |
| `lease_time_secs` | `spec.dhcp.leaseTime` (default 3600) |
| `next_server` | `spec.dhcp.nextServer` (PXE TFTP) |
| `boot_file` | `spec.dhcp.bootFile` (PXE BIOS) |
| `boot_file_efi` | `spec.dhcp.bootFileEFI` (PXE UEFI) |
| `root_path` | Auto-set for data networks (iSCSI baremetalservices target) |

## Current Networks

| Network | Type | Zone | DNS Server | DHCP |
|---------|------|------|-----------|------|
| gt | management | gt.lo | 192.168.200.199 | no |
| g8 | data | g8.lo | 192.168.8.252 | yes |
| g9 | wifi | g9.lo | 192.168.9.252 | yes |
| g10 | data | g10.lo | 192.168.10.252 | yes |
| g11 | ipmi | g11.lo | 192.168.11.252 | yes |
| gw | external | gw.lo | 192.168.1.252 | yes |

## Domain Search (Option 119)

mkube sets `domain_search` to ALL zones — local zone first, then all peer zones sorted alphabetically. For example, g10 gets `["g10.lo", "g11.lo", "g8.lo", "g9.lo", "gt.lo", "gw.lo"]`. This ensures clients can resolve bare hostnames across all networks without DNS scoping issues.

### DHCP Option Behavior

- **Option 119 (domain search)**: Sent when `domain_search` is set. Provides hostname completion domains.
- **Option 15 (domain name)**: Suppressed when `domain_search` is non-empty. Sent using `domain` field when `domain_search` is absent.
- **Option 6 (DNS servers)**: Always sent from `dns_servers` field.

Option 15 causes systemd-resolved to scope the DNS server to that single domain. Option 119 avoids this side-effect while still providing hostname completion.

## Forward Zones

Cross-network DNS resolution requires forward zones on each microdns instance. These are auto-computed from all peer networks when the TOML config is generated via `GET /api/v1/networks/{name}/config`. Each managed microdns forwards queries for peer zones to the peer's DNS server.

**Note:** Forward zones must be seeded via `seedDNSConfig()` — they are pushed to microdns via the REST API, not baked into the TOML config.

## REST API Examples

### Create pool
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
    "domain_search": ["g10.lo"],
    "lease_time_secs": 600
  }'
```

### Update existing pool
```bash
curl -X PATCH http://192.168.10.252:8080/api/v1/dhcp/pools/{id} \
  -H 'Content-Type: application/json' \
  -d '{
    "dns_servers": ["192.168.10.252"],
    "domain_search": ["g10.lo"]
  }'
```

## Reservation Fields

DHCP reservations are synced from BMH objects to Network CRDs. mkube populates these fields when creating reservations:

| Field | Source |
|-------|--------|
| `mac` | BMH `spec.bootMACAddress` or `spec.bmc.mac` |
| `ip` | BMH `spec.ip` or `spec.bmc.address` |
| `hostname` | BMH `spec.hostname` or `spec.bmc.hostname` |
| `gateway` | Network CRD `spec.gateway` |
| `dns_servers` | Network CRD `spec.dns.server` |
| `domain` | Network CRD `spec.dns.zone` |
| `root_path` | BMH `spec.disk` iSCSI target (if set) |
| `next_server` | Network CRD `spec.dhcp.nextServer` |
| `boot_file` | Network CRD `spec.dhcp.bootFile` |
| `boot_file_efi` | Network CRD `spec.dhcp.bootFileEFI` |

### Pool Inheritance

All reservation fields in microdns are optional (`Option<T>`). When a field is not set on the reservation, the pool default is used automatically. This applies to every DHCP option:

- `gateway` (option 3)
- `dns_servers` (option 6)
- `domain` (option 15)
- `domain_search` (option 119)
- `lease_time_secs`
- `ntp_servers` (option 42)
- `mtu` (option 26)
- `next_server` / `boot_file` / `boot_file_efi` (PXE options)
- `root_path` (option 17)
- `static_routes` (option 121)
- `log_server`, `time_offset`, `wpad_url`

The inheritance pattern in `build_response()`:
```rust
let gw = db_res
    .and_then(|r| r.gateway.as_ref())
    .unwrap_or(pool.gateway);  // fallback to pool default
```

This means a minimal reservation only needs `mac`, `ip`, and `hostname` — everything else is inherited from the pool. mkube explicitly sets gateway/dns/domain on reservations for robustness, but microdns would supply them from the pool regardless.

### Reserved IPs Outside Pool Range

When a reserved IP falls outside all configured pool ranges, microdns falls back to the first pool's defaults. This ensures reserved hosts always get complete network configuration even if their IP isn't in any pool range.
