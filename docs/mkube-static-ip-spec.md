# mkube Static IP Assignment Spec

## Problem

Containers on the gt network (192.168.200.0/24) get dynamically assigned IPs from mkube on every restart or redeployment. This causes:

1. **IP drift** — the same container gets a different IP each time (e.g., netwatch was observed at .88, then .98 within minutes)
2. **Stale DNS** — microdns auto-registers the new IP but the old A record lingers, creating duplicates or confusion
3. **No stable addressing** — other services can't rely on IP-based ACLs, firewall rules, or monitoring targets

## Proposed Solution

mkube should support a static IP field in container specs. When `ip` is set, mkube must assign exactly that IP to the container's network interface — not a dynamic one from the pool.

### Container Spec Change

Add an optional `ip` field to the container network configuration:

```yaml
apiVersion: v1
kind: Container
metadata:
  name: nats
  namespace: nats
spec:
  network: gt
  ip: 192.168.200.10    # <-- new field: static IP assignment
  image: registry.gt.lo:5000/nats:latest
  ...
```

When `ip` is present:
- mkube MUST assign this exact IP to the container
- If the IP is already in use by another container, the deployment MUST fail with a clear error (not silently assign a different IP)
- The IP must be within the network's subnet range
- mkube must track static IPs separately from the dynamic pool to avoid collisions

When `ip` is absent:
- Current behavior (dynamic assignment from pool) is preserved

### Recommended Static IP Allocations — gt Network

Based on current container inventory, the following static assignments are recommended. IPs are grouped by function with room for growth.

#### Infrastructure (192.168.200.1–9)

| IP | Container | Namespace | Notes |
|----|-----------|-----------|-------|
| .1 | rose1 | — | Router (not a container) |
| .2 | mkube | mkube | Cluster manager |
| .3 | registry | registry | Container image registry |
| .4 | *(reserved)* | — | Future infra |
| .5 | *(reserved)* | — | Future infra |
| .6 | registry-stormbase | registry-stormbase | Base image registry |

#### Core Services (192.168.200.10–19)

| IP | Container | Namespace | Notes |
|----|-----------|-----------|-------|
| .10 | nats | nats | Message bus |
| .11 | miniminio | miniminio | S3-compatible object store |
| .12 | git | git | Git server (rust4git) |
| .13 | minio | minio | S3-compatible object store |
| .14 | console | console | Web console |

#### Applications (192.168.200.20–49)

| IP | Container | Namespace | Notes |
|----|-----------|-----------|-------|
| .20 | cloudid | cloudid | Cloud identity service |
| .21 | netwatch | netwatch | Network monitor |
| .22 | pvc-test | pvc-test | PVC testing |

#### DNS (192.168.200.199)

| IP | Container | Namespace | Notes |
|----|-----------|-----------|-------|
| .199 | microdns | dns | DNS server (static, already pinned) |

#### Dynamic Pool (192.168.200.100–198)

IPs .100–.198 remain available for containers without static assignments.

### Scope

This spec covers the gt network only, but the same mechanism should work for any mkube-managed network. The g10 and g11 networks use microdns DHCP for IP assignment and don't need this — their containers already get stable IPs via DHCP reservations.

### Implementation Notes

- mkube's IP allocator needs a "reserved" set that is excluded from dynamic assignment
- On container start, if `ip` is specified, skip the allocator and use the static IP directly
- Persist static IP mappings so they survive mkube restarts
- The static IP field should be validated at deploy time (within subnet, not a broadcast/network address, not the gateway)

### Migration

For existing containers, update their specs one at a time to add the `ip` field matching their current assignment. On next restart they'll get the pinned IP instead of a random one.
