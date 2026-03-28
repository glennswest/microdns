# mkube Static IP Assignment Spec

## Problem

Containers on the gt network (192.168.200.0/24) get dynamically assigned IPs from mkube on every restart or redeployment. This causes:

1. **IP drift** — the same container gets a different IP each time (e.g., netwatch was observed at .88, then .98 within minutes)
2. **Stale DNS** — microdns auto-registers the new IP but the old A record lingers, creating duplicates or confusion
3. **No stable addressing** — other services can't rely on IP-based ACLs, firewall rules, or monitoring targets

## Proposed Solution

mkube should maintain a **DNS reservation table** — a mapping of container name to static IP. When a container starts, mkube looks up the container in this table and assigns the reserved IP. The table is managed by mkube internally (not in container YAML specs) to avoid collision issues from users hardcoding IPs in manifests.

### How It Works

1. mkube maintains an internal reservation table (persisted in its database)
2. When a container is deployed, mkube checks if the container name has a reserved IP
3. If yes → assign that exact IP, register it in DNS
4. If no → assign from the dynamic pool as today, then auto-reserve that IP for next time
5. Reserved IPs are excluded from the dynamic pool to prevent collisions
6. The reservation table is the source of truth — no IP field in container YAML

### API

```
GET  /api/v1/ipreservations                  — list all reservations
POST /api/v1/ipreservations                  — create: {"name": "nats", "network": "gt", "ip": "192.168.200.2"}
PUT  /api/v1/ipreservations/{name}           — update IP
DELETE /api/v1/ipreservations/{name}         — release reservation
```

## Static IP Allocations — gt Network (192.168.200.0/24)

All containers assigned sequentially. .1 is the router, .199 is DNS.

| IP | Container | Notes |
|----|-----------|-------|
| .1 | rose1 | Router (not a container) |
| .2 | mkube | Cluster manager |
| .3 | registry | Container image registry |
| .4 | registry-stormbase | Base image registry |
| .5 | nats | Message bus |
| .6 | minio | S3 object store |
| .7 | miniminio | S3 object store (small) |
| .8 | git (rust4git) | Git server |
| .9 | console | Web console |
| .10 | cloudid | Cloud identity service |
| .11 | netwatch | Network monitor |
| .12 | pvc-test | PVC testing |
| .199 | microdns | DNS server (already pinned) |

Dynamic pool: .100–.198 for containers without reservations.

## Scope

This spec covers the gt network, but the mechanism should work for any mkube-managed network. The g10 and g11 networks use microdns DHCP reservations for stable IPs and don't need this.

## Implementation Notes

- The reservation table should be persisted (survives mkube restarts)
- On container start: lookup reservation → assign IP → register DNS
- On container stop: keep the reservation (IP stays reserved even when container is down)
- New containers without a reservation auto-get one after first dynamic assignment
- Reserved IPs must be excluded from the dynamic allocator to prevent collisions
- Provide a CLI or API to view/manage reservations
