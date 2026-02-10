# MicroDNS

A single-binary DNS infrastructure server replacing PowerDNS + pdnsloadbalancer. Provides authoritative DNS, recursive DNS with forward-and-fallback, DNS-based load balancing, DHCPv4/v6/SLAAC, and IPAM — all backed by an embedded database with zero external dependencies.

Designed for an instance-per-network federated topology where each subnet runs its own MicroDNS instance, with cross-instance forwarding and automatic failover.

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                      MicroDNS Binary                     │
│                                                          │
│  ┌────────────┐  ┌────────────┐  ┌────────────────────┐  │
│  │ Auth DNS   │  │ Recursor   │  │ Load Balancer      │  │
│  │ :53        │  │ :53/:5353  │  │ Health Monitor     │  │
│  └────────────┘  └────────────┘  └────────────────────┘  │
│  ┌────────────┐  ┌────────────┐  ┌────────────────────┐  │
│  │ DHCPv4 :67 │  │ DHCPv6     │  │ SLAAC RA           │  │
│  │            │  │ :547       │  │                    │  │
│  └────────────┘  └────────────┘  └────────────────────┘  │
│  ┌────────────┐  ┌────────────┐  ┌────────────────────┐  │
│  │ REST API   │  │ gRPC API   │  │ Dashboard + WS     │  │
│  │ :8080      │  │ :50051     │  │ :8080/dashboard    │  │
│  └────────────┘  └────────────┘  └────────────────────┘  │
│  ┌─────────────────────────────────────────────────────┐  │
│  │              redb (embedded database)               │  │
│  └─────────────────────────────────────────────────────┘  │
│  ┌────────────────────┐  ┌─────────────────────────────┐  │
│  │ Federation Agent   │  │ Message Bus (Kafka/NoOp)    │  │
│  └────────────────────┘  └─────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

### Multi-Instance Topology

```
          ┌─────────────────┐
          │  main (gw.lo)   │──── bridge-lan (192.168.1.0/24)
          │  .1.199         │
          └───────┬─────────┘
                  │ forward zones
    ┌─────────────┼─────────────┐
    ▼             ▼             ▼
┌─────────┐ ┌─────────┐ ┌─────────┐
│ g10     │ │ g11     │ │ gt      │
│ .10.199 │ │ .11.199 │ │ .200.199│
└─────────┘ └─────────┘ └─────────┘
  bridge       bridge-boot  bridge-gt
```

Each instance is authoritative for its own subnet zones. Cross-subnet queries are forwarded peer-to-peer. If a peer goes down, the forwarding instance falls back to a local copy of the zone data (served as non-authoritative).

## Features

- **Authoritative DNS** — A, AAAA, CNAME, MX, NS, PTR, SOA, SRV, TXT, CAA records
- **Recursive DNS** — Cache, forward zones, upstream forwarding (UDP + TCP)
- **Forward-with-Fallback** — Forward to peer instances; serve local copy if peer is down (AA=0)
- **DNS Load Balancing** — Health-checked records with ping/HTTP/HTTPS/TCP probes
- **DHCPv4** — DORA flow, pools, static reservations, PXE boot (next-server/boot-file)
- **DHCPv6** — Stateful address assignment, prefix delegation
- **SLAAC** — Router Advertisement daemon
- **DNS Auto-Registration** — DHCP leases automatically create A/AAAA + PTR records
- **IPAM** — IP address management for container workloads
- **Peer Connectivity Testing** — Built-in endpoint to probe DNS/HTTP to all peers
- **Federation** — Leaf/coordinator agents with heartbeat and config sync via Kafka
- **Dashboard** — Embedded HTML + WebSocket real-time dashboard
- **gRPC API** — Zone, Record, Lease, Cluster, and Health services
- **REST API** — Full CRUD for zones, records, leases, IPAM, connectivity
- **Embedded Database** — redb with ACID transactions, no external dependencies
- **Static Binary** — Single musl-linked binary, runs from scratch container

## Build

### Requirements

- Rust 1.88+ (edition2024 support required)
- `protobuf-compiler` (for gRPC codegen)
- Docker (for container builds)

### Local Build

```bash
cargo build --release
```

### Docker Build (ARM64)

```bash
# Native ARM64 on Apple Silicon — no cross-compile needed
docker build -t microdns:arm64 .
```

The Dockerfile uses a multi-stage build: `rust:1.88-bookworm` builder with musl target producing a static binary, copied into a `scratch` runtime image.

### Build for RouterOS

```bash
# Build container image and save as tarball
scripts/build-container.sh

# Or manually:
docker build -t microdns:arm64 .
docker save microdns:arm64 -o microdns-arm64.tar
```

### Run Tests

```bash
cargo test --workspace
```

## Usage

```bash
microdns --config /etc/microdns/microdns.toml
```

### Ports

| Port | Protocol | Service |
|------|----------|---------|
| 53 | UDP/TCP | DNS (auth or recursor) |
| 5353 | UDP/TCP | DNS (recursor, when auth uses 53) |
| 67 | UDP | DHCPv4 |
| 547 | UDP | DHCPv6 |
| 8080 | TCP | REST API + Dashboard |
| 50051 | TCP | gRPC API |

## Configuration

TOML format with extensive serde defaults. Minimal config:

```toml
[instance]
id = "my-dns"
mode = "standalone"    # standalone | leaf | coordinator

[dns.recursor]
enabled = true
listen = "0.0.0.0:53"

[database]
path = "/data/microdns.redb"

[logging]
level = "info"         # debug | info | warn | error
format = "json"        # json | text
```

### Full Configuration Reference

```toml
[instance]
id = "microdns-main"
mode = "standalone"

# Peer instances for connectivity testing
[[instance.peers]]
id = "microdns-g10"
addr = "192.168.10.199"
dns_port = 53            # default: 53
http_port = 8080         # default: 8080

# Federation coordinator (leaf mode only)
[coordinator]
endpoint = "grpc://coordinator:50051"
heartbeat_interval_secs = 10
report_interval_secs = 30

# --- DNS ---

[dns.auth]
enabled = true
listen = "0.0.0.0:53"
zones = ["example.com", "1.168.192.in-addr.arpa"]

[dns.recursor]
enabled = true
listen = "0.0.0.0:53"   # or :5353 if auth uses :53
cache_size = 10000

[dns.recursor.forward_zones]
"g10.lo" = ["192.168.10.199:53"]
"g11.lo" = ["192.168.11.199:53"]

[dns.loadbalancer]
enabled = true
check_interval_secs = 10
default_probe = "ping"   # ping | http | https | tcp

# --- DHCP ---

[dhcp.v4]
enabled = true
interface = "eth0"

[[dhcp.v4.pools]]
range_start = "192.168.1.100"
range_end = "192.168.1.200"
subnet = "192.168.1.0/24"
gateway = "192.168.1.1"
dns = ["192.168.1.199"]
domain = "gw.lo"
lease_time_secs = 3600
next_server = "192.168.1.5"   # PXE boot
boot_file = "pxelinux.0"

[[dhcp.v4.reservations]]
mac = "AA:BB:CC:DD:EE:FF"
ip = "192.168.1.50"
hostname = "server1"

[dhcp.v6]
enabled = true
interface = "eth0"

[[dhcp.v6.pools]]
prefix = "2001:db8::"
prefix_len = 64
dns = ["2001:db8::1"]
domain = "example.com"
lease_time_secs = 3600

[dhcp.slaac]
enabled = true
interface = "eth0"
prefix = "2001:db8::"
prefix_len = 64

[dhcp.dns_registration]
enabled = true
forward_zone = "gw.lo"
reverse_zone_v4 = "1.168.192.in-addr.arpa"
reverse_zone_v6 = ""
default_ttl = 300

# --- Messaging ---

[messaging]
backend = "noop"          # noop | kafka
brokers = ["kafka:9092"]
topic_prefix = "microdns"

# --- IPAM ---

[ipam]
enabled = true

[[ipam.pools]]
name = "containers"
subnet = "192.168.200.0/24"
range_start = "192.168.200.10"
range_end = "192.168.200.199"
gateway = "192.168.200.1"
bridge = "bridge-gt"

# --- API ---

[api.rest]
enabled = true
listen = "0.0.0.0:8080"
api_key = "secret"        # optional

[api.grpc]
enabled = true
listen = "0.0.0.0:50051"

# --- Storage ---

[database]
path = "/data/microdns.redb"

[logging]
level = "info"
format = "json"
```

## REST API

Base path: `/api/v1`

### Zones

```
GET    /api/v1/zones                          List all zones
POST   /api/v1/zones                          Create zone
GET    /api/v1/zones/{id}                     Get zone
DELETE /api/v1/zones/{id}                     Delete zone
POST   /api/v1/zones/transfer                 AXFR zone transfer from primary
```

Create zone:
```json
{
  "name": "example.com",
  "default_ttl": 300,
  "soa": {
    "mname": "ns1.example.com",
    "rname": "admin.example.com"
  }
}
```

### Records

```
GET    /api/v1/zones/{zone_id}/records              List records
POST   /api/v1/zones/{zone_id}/records              Create record
GET    /api/v1/zones/{zone_id}/records/{record_id}  Get record
PUT    /api/v1/zones/{zone_id}/records/{record_id}  Update record
DELETE /api/v1/zones/{zone_id}/records/{record_id}  Delete record
```

Record data types:
```json
{"type": "A",     "data": "192.168.1.10"}
{"type": "AAAA",  "data": "2001:db8::1"}
{"type": "CNAME", "data": "target.example.com"}
{"type": "MX",    "data": {"preference": 10, "exchange": "mail.example.com"}}
{"type": "NS",    "data": "ns1.example.com"}
{"type": "PTR",   "data": "host.example.com"}
{"type": "SRV",   "data": {"priority": 10, "weight": 60, "port": 5060, "target": "sip.example.com"}}
{"type": "TXT",   "data": "v=spf1 include:example.com ~all"}
{"type": "CAA",   "data": {"flags": 0, "tag": "issue", "value": "letsencrypt.org"}}
```

Create record with health check:
```json
{
  "name": "web",
  "ttl": 300,
  "data": {"type": "A", "data": "192.168.1.10"},
  "health_check": {
    "probe_type": "http",
    "interval_secs": 10,
    "timeout_secs": 5,
    "unhealthy_threshold": 3,
    "healthy_threshold": 2,
    "endpoint": ":8080/health"
  }
}
```

### Leases

```
GET    /api/v1/leases                         List active DHCP leases
```

### Health

```
GET    /api/v1/health                         Instance health check
```

Response:
```json
{"status": "ok", "version": "0.1.0", "zones": 12}
```

### Connectivity

```
GET    /api/v1/connectivity                   Probe all peer instances
```

Response:
```json
{
  "instance_id": "microdns-main",
  "peers": [
    {
      "id": "microdns-g10",
      "addr": "192.168.10.199",
      "dns_udp": {"ok": true, "latency_ms": 21.5, "error": null},
      "dns_tcp": {"ok": true, "latency_ms": 23.1, "error": null},
      "http":    {"ok": true, "latency_ms": 0.8, "error": null}
    }
  ]
}
```

### Cluster

```
GET    /api/v1/cluster/status                 Federation cluster status
```

### IPAM

```
GET    /api/v1/ipam/pools                     List IP pools
GET    /api/v1/ipam/allocations               List allocations
POST   /api/v1/ipam/allocate                  Allocate IP
DELETE /api/v1/ipam/allocations/{id}          Release IP
```

Allocate:
```json
{"pool": "containers", "container": "my-app"}
```

Response:
```json
{"id": "...", "ip": "192.168.200.15", "pool": "containers", "gateway": "192.168.200.1", "bridge": "bridge-gt", "subnet": "192.168.200.0/24", "container": "my-app"}
```

## gRPC API

Port 50051. Proto definition in `proto/microdns.proto`.

| Service | Methods |
|---------|---------|
| ZoneService | ListZones, GetZone, CreateZone, DeleteZone |
| RecordService | ListRecords, CreateRecord, UpdateRecord, DeleteRecord |
| LeaseService | ListLeases |
| ClusterService | GetClusterStatus, Heartbeat, PushConfig |
| HealthService | GetHealthStatus |

## Forward-with-Fallback

When a zone exists both locally and in the forward table, the resolver:

1. **Skips** local auth for forwarded zones (lets them reach the forward step)
2. **Forwards** to the peer instance (authoritative, AA=1)
3. **Falls back** to local data if all forward servers fail (non-authoritative, AA=0)

```
dig @main server1.g10.lo    # g10 up   → AA=1 (forwarded from g10)
dig @main server1.g10.lo    # g10 down → AA=0 (local fallback on main)
```

This provides redundancy: clients always get an answer even when a subnet instance is down.

## Load Balancer

Records with `health_check` config are monitored by the health monitor. Probe types:

| Probe | Description | Endpoint Format |
|-------|-------------|-----------------|
| `ping` | TCP connect to port 80/443 | — |
| `http` | HTTP GET, expects 2xx | `:8080/health` |
| `https` | HTTPS GET, expects 2xx | `:443/health` |
| `tcp` | TCP connect test | `:8080` |

Unhealthy records are automatically excluded from DNS responses. The state machine uses configurable thresholds to prevent flapping.

## Scripts

| Script | Purpose |
|--------|---------|
| `scripts/build-container.sh` | Build ARM64 binary + RouterOS tarball |
| `scripts/setup-rose1.sh` | Deploy/manage containers on MikroTik RouterOS |
| `scripts/import-zones.sh` | Import zones from PowerDNS |
| `scripts/clone-dhcp-reservations.sh` | Clone DHCP reservations from MikroTik routers |

### Deploy to RouterOS

```bash
# Build
scripts/build-container.sh

# First-time deploy (creates bridges, mounts, veths, containers)
scripts/setup-rose1.sh deploy

# Start all instances
scripts/setup-rose1.sh start

# Update after code changes
scripts/setup-rose1.sh redeploy
```

### Import Zones from PowerDNS

```bash
scripts/import-zones.sh \
  --target http://192.168.1.199:8080 \
  --pdns-key "your-api-key" \
  --zones "gw.lo,g10.lo,1.168.192.in-addr.arpa"
```

## Kubernetes Deployment

Manifests in `k8s/`:

```bash
kubectl apply -f k8s/namespace.yaml
kubectl apply -f k8s/rbac.yaml
kubectl apply -f k8s/configmap.yaml
kubectl apply -f k8s/deployment-leaf.yaml       # DaemonSet, one per node
kubectl apply -f k8s/deployment-coordinator.yaml # Single replica
kubectl apply -f k8s/service.yaml
```

- **Leaf instances** run as a DaemonSet with `hostNetwork: true` for DNS port binding
- **Coordinator** runs as a single-replica Deployment
- Federation uses Kafka for heartbeat and config sync between instances

## Security

MicroDNS includes defense-in-depth security hardening across all network-facing services.

### Authentication

- **API key enforcement** — Set `api_key` in config; all REST, gRPC, and WebSocket endpoints require `X-API-Key` header. `/health` and `/dashboard` are exempt.
- **No default credentials** — API key is optional; when unset, endpoints are open (suitable for trusted networks).

### Input Validation

- **DNS name validation** — Zone and record names are validated against RFC 1035 (max 253 chars, labels max 63 chars, alphanumeric + hyphens only) on all REST and gRPC create/update endpoints.
- **Request body limits** — REST: 1 MB max body size. gRPC: 1 MB max message size.
- **gRPC data_json limit** — Record data payloads capped at 10 KB.
- **Federation payload limits** — Zone sync and config push messages capped at 10 MB.

### Rate Limiting & Resource Protection

| Resource | Limit | Behavior when exceeded |
|----------|-------|----------------------|
| TCP connections (auth DNS) | 1,000 concurrent | New connections rejected |
| TCP connections (recursor) | 1,000 concurrent | New connections rejected |
| UDP query tasks (recursor) | 10,000 concurrent | Queries dropped |
| WebSocket connections | 100 concurrent | Returns 503 |
| TCP connection timeout | 30 seconds | Connection terminated |
| AXFR transfer records | 100,000 max | Transfer aborted |
| AXFR transfer size | 100 MB max | Transfer aborted |
| List endpoint pagination | Default 100, max 1,000 | Use `?offset=N&limit=N` |
| WebSocket message size | 2 MB max | Oversized messages skipped |

### Error Handling

- **Sanitized responses** — Internal errors return generic "internal server error" to clients. Full error details are logged server-side only.
- **No unsafe code** — Zero `unsafe` blocks across the entire codebase.
- **No panics on untrusted input** — All network input parsing uses proper error handling with no `unwrap()` on external data.

### Lease Management

- **Automatic cleanup** — Background task purges expired DHCP leases every 5 minutes (24-hour retention after expiry) to prevent unbounded database growth.

### DNS Protocol Safety

- **Bounded allocations** — UDP buffers are fixed-size (4,096 bytes DNS, 1,500 bytes DHCP).
- **Cache limits** — Recursive resolver cache enforces configurable `max_size` with TTL-based eviction.
- **DHCP packet validation** — Comprehensive bounds checking on all DHCPv4/v6 option parsing.

## Crate Structure

```
crates/
  microdns-core/       Core types, config, redb database, error handling
  microdns-auth/       Authoritative DNS server (hickory-dns)
  microdns-recursor/   Recursive resolver with cache, forwarding, TCP
  microdns-lb/         Load balancer health monitor and probes
  microdns-dhcp/       DHCPv4, DHCPv6, SLAAC, DNS auto-registration
  microdns-msg/        Message bus abstraction (Kafka, NoOp)
  microdns-federation/ Leaf/coordinator agents, heartbeat, config sync
  microdns-api/        REST (axum) + gRPC (tonic) + dashboard + WebSocket
```

## License

MIT
