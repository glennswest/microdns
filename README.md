# MicroDNS

A single-binary DNS infrastructure server replacing PowerDNS + pdnsloadbalancer. Provides authoritative DNS, recursive DNS with forward-and-fallback, DNS-based load balancing, DHCPv4/v6/SLAAC, and IPAM — all backed by an embedded database with zero external dependencies.

Designed for an instance-per-network federated topology where each subnet runs its own MicroDNS instance, with cross-instance forwarding and automatic failover.

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                     stormd (PID 1)                        │
│         process supervisor, SSH, web dashboard            │
│  ┌────────────────────────────────────────────────────┐   │
│  │                  MicroDNS Binary                    │   │
│  │                                                     │   │
│  │  ┌────────────┐  ┌────────────┐  ┌──────────────┐  │   │
│  │  │ Auth DNS   │  │ Recursor   │  │ Load Balancer│  │   │
│  │  │ :53        │  │ :53/:5353  │  │ Health Mon   │  │   │
│  │  └────────────┘  └────────────┘  └──────────────┘  │   │
│  │  ┌────────────┐  ┌────────────┐  ┌──────────────┐  │   │
│  │  │ DHCPv4 :67 │  │ DHCPv6     │  │ SLAAC RA     │  │   │
│  │  │            │  │ :547       │  │              │  │   │
│  │  └────────────┘  └────────────┘  └──────────────┘  │   │
│  │  ┌────────────┐  ┌────────────┐  ┌──────────────┐  │   │
│  │  │ REST API   │  │ gRPC API   │  │ Dashboard+WS │  │   │
│  │  │ :8080      │  │ :50051     │  │ :80          │  │   │
│  │  └────────────┘  └────────────┘  └──────────────┘  │   │
│  │  ┌─────────────────────────────────────────────┐   │   │
│  │  │           redb (embedded database)           │   │   │
│  │  └─────────────────────────────────────────────┘   │   │
│  │  ┌──────────────────┐  ┌────────────────────────┐  │   │
│  │  │ Federation Agent │  │ Message Bus (NATS/NoOp)│  │   │
│  │  └──────────────────┘  └────────────────────────┘  │   │
│  └────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

### Container Runtime

MicroDNS runs inside a **stormdbase** container image instead of bare scratch. [stormd](https://github.com/glennswest/stormd) provides:

- **Process supervision** — automatic restart on crash with configurable thresholds
- **Liveness probes** — TCP port 53 health check, restarts microdns if DNS stops responding
- **SSH access** — built-in SSH server for debugging running containers
- **Web dashboard** — process status, memory charts, restart history on port 9080
- **Structured logging** — stdout/stderr capture with severity detection
- **Busybox commands** — 63 built-in Unix commands (ls, cat, grep, curl, dig, etc.)

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
│ .10.252 │ │ .11.252 │ │ .200.199│
└─────────┘ └─────────┘ └─────────┘
  bridge       bridge-boot  bridge-gt
```

Each instance is authoritative for its own subnet zones. Cross-subnet queries are forwarded peer-to-peer. If a peer goes down, the forwarding instance falls back to a local copy of the zone data (served as non-authoritative).

### Current Deployment

| Instance | Network | IP | Zone | DHCP |
|----------|---------|-----|------|------|
| main | gw | 192.168.1.199 | gw.lo | no |
| g10 | data | 192.168.10.252 | g10.lo | yes |
| g11 | ipmi | 192.168.11.252 | g11.lo | yes |
| gt | mgmt | 192.168.200.199 | gt.lo | no |

All instances are managed by **mkube** which auto-deploys from the container registry.

## Features

- **Authoritative DNS** — A, AAAA, CNAME, MX, NS, PTR, SOA, SRV, TXT, CAA records
- **Recursive DNS** — Cache, forward zones, upstream forwarding (UDP + TCP)
- **Forward-with-Fallback** — Forward to peer instances; serve local copy if peer is down (AA=0)
- **DNS Load Balancing** — Health-checked records with ping/HTTP/HTTPS/TCP probes
- **Correct NOERROR/NXDOMAIN** — Returns NOERROR with empty answer when name exists but queried type has no records (required for systemd-resolved parallel A+AAAA lookups)
- **DHCPv4** — DORA flow, pools, static reservations, PXE/iPXE boot support
- **DHCP Extended Options** — NTP (42), MTU (26), domain search (119), classless static routes (121), log server (7), time offset (2), WPAD (252)
- **DHCP Option 15/119 Handling** — Option 15 suppressed when domain search (119) is configured to prevent systemd-resolved DNS scoping
- **DHCP Reservation Inheritance** — Reservations inherit all extended options from pool when not explicitly overridden
- **DHCPv6** — Stateful address assignment, prefix delegation
- **SLAAC** — Router Advertisement daemon
- **DNS Auto-Registration** — DHCP leases automatically create A/AAAA + PTR records with deduplication
- **IPAM** — IP address management for container workloads
- **Database-Driven Config** — All pools, reservations, forwarders stored in redb, managed via REST API
- **TOML Bootstrap Migration** — One-time import from TOML config on first boot
- **Peer Connectivity Testing** — Built-in endpoint to probe DNS/HTTP to all peers
- **Federation** — Leaf/coordinator agents with heartbeat and config sync via NATS
- **Dashboard** — 7-tab SPA (Overview, DNS, LB, DHCP, Events, Logs, Peers)
- **SSE Watch** — `GET /api/v1/watch?types=dhcp,dns,zones,records,leases` for real-time event streams
- **gRPC API** — Zone, Record, Lease, Cluster, and Health services
- **REST API** — Full CRUD for zones, records, pools, reservations, forwarders, leases, IPAM
- **Embedded Database** — redb with ACID transactions, no external dependencies
- **Static Binary** — Single musl-linked binary, runs in stormdbase container

## Build

### Requirements

- Rust 1.88+ (edition2024 support required)
- `protobuf-compiler` (for gRPC codegen)
- `podman` (for container builds)
- ARM64 cross-compile toolchain (`aarch64-linux-musl-gcc`)

### Local Build

```bash
cargo build --release
```

### Cross-Compile + Container Build + Deploy

```bash
# One-step: cross-compile ARM64, build container image, push to registry
# mkube auto-deploys within ~2 minutes
scripts/build-and-push.sh          # push as :edge (default)
scripts/build-and-push.sh latest   # push as :latest
```

The script:
1. Cross-compiles for `aarch64-unknown-linux-musl`
2. Builds a container image using `stormdbase` as base (via `Dockerfile.scratch`)
3. Pushes to `registry.gt.lo:5000/microdns:edge`
4. mkube watches the registry and auto-deploys new images

### Run Tests

```bash
cargo test --workspace
```

## Usage

```bash
microdns --config /etc/microdns/microdns.toml
```

Or with CLI bootstrap flags:

```bash
microdns --listen-dns 0.0.0.0:53 --data-dir /data --mode standalone
```

### Ports

| Port | Protocol | Service |
|------|----------|---------|
| 53 | UDP/TCP | DNS (auth or recursor) |
| 5353 | UDP/TCP | DNS (recursor, when auth uses 53) |
| 67 | UDP | DHCPv4 |
| 547 | UDP | DHCPv6 |
| 80 | TCP | Dashboard + WebSocket |
| 8080 | TCP | REST API |
| 50051 | TCP | gRPC API |
| 9080 | TCP | stormd web dashboard |
| 22 | TCP | stormd SSH |

## Configuration

### Database-Driven Config

All runtime configuration is stored in the redb database and managed via REST API:

- **DHCP pools** — `POST/GET/PATCH/DELETE /api/v1/dhcp/pools`
- **DHCP reservations** — `POST/GET/PATCH/DELETE /api/v1/dhcp/reservations`
- **DNS forwarders** — `POST/GET/DELETE /api/v1/dns/forwarders`
- **Instance config** — `GET/PATCH /api/v1/dhcp/config`

Changes take effect immediately — redb is memory-mapped, no reload needed.

### TOML Bootstrap

TOML config is used for initial bootstrap only. On first boot, pools/reservations/forwarders are migrated from TOML to the database. After that, TOML is only needed for listen addresses and instance identity.

Minimal config:

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
addr = "192.168.10.252"
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
"g10.lo" = ["192.168.10.252:53"]
"g11.lo" = ["192.168.11.252:53"]

[dns.loadbalancer]
enabled = true
check_interval_secs = 10
default_probe = "ping"   # ping | http | https | tcp

# --- DHCP ---

[dhcp.v4]
enabled = true
interface = "eth0"
mode = "normal"          # normal | gateway (relay-only with veth)

[[dhcp.v4.pools]]
range_start = "192.168.1.100"
range_end = "192.168.1.200"
subnet = "192.168.1.0/24"
gateway = "192.168.1.1"
dns_servers = ["192.168.1.199"]
domain = "gw.lo"
domain_search = ["gw.lo", "g10.lo", "g11.lo", "gt.lo"]
lease_time_secs = 3600
next_server = "192.168.1.5"       # PXE TFTP server
boot_file = "pxelinux.0"          # PXE BIOS boot file
boot_file_efi = "ipxe.efi"        # PXE UEFI boot file
ntp_servers = ["192.168.1.1"]     # option 42
mtu = 1500                        # option 26
log_server = "192.168.1.5"        # option 7
time_offset = -18000              # option 2
wpad_url = "http://wpad/wpad.dat" # option 252

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
backend = "noop"          # noop | nats
nats_url = "nats://localhost:4222"
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
dashboard_listen = "0.0.0.0:80"
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

### stormd Config

The stormd process supervisor is configured via `config/stormd.toml`:

```toml
[general]
name = "microdns"

[api]
bind = "0.0.0.0:9080"

[ssh]
enabled = true
bind = "0.0.0.0:22"
password = "stormd"

[[process]]
name = "microdns"
command = "/microdns"
args = ["--config", "/etc/microdns/microdns.toml"]
on_failure = "restart"
on_exit = "restart"
restart_delay_secs = 2

[process.liveness]
type = "tcp"
port = 53
interval_secs = 10
failure_threshold = 3
initial_delay_secs = 5
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
    "endpoint": ":80/health"
  }
}
```

### DHCP Pools

```
GET    /api/v1/dhcp/pools                     List pools
POST   /api/v1/dhcp/pools                     Create pool
GET    /api/v1/dhcp/pools/{id}                Get pool
PATCH  /api/v1/dhcp/pools/{id}                Update pool
DELETE /api/v1/dhcp/pools/{id}                Delete pool
GET    /api/v1/dhcp/pools/{id}/routes         List static routes
POST   /api/v1/dhcp/pools/{id}/routes         Add static route
DELETE /api/v1/dhcp/pools/{id}/routes/{rid}   Delete static route
```

### DHCP Reservations

```
GET    /api/v1/dhcp/reservations              List reservations
POST   /api/v1/dhcp/reservations              Create reservation
GET    /api/v1/dhcp/reservations/{id}         Get reservation
PATCH  /api/v1/dhcp/reservations/{id}         Update reservation
DELETE /api/v1/dhcp/reservations/{id}         Delete reservation
```

### DNS Forwarders

```
GET    /api/v1/dns/forwarders                 List forward zones
POST   /api/v1/dns/forwarders                 Create forward zone
DELETE /api/v1/dns/forwarders/{zone}          Delete forward zone
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

### Watch (SSE)

```
GET    /api/v1/watch?types=dhcp,dns,zones,records,leases
```

Real-time event stream via Server-Sent Events. Filter by event type.

### Connectivity

```
GET    /api/v1/connectivity                   Probe all peer instances
```

### DHCP Status

```
GET    /api/v1/dhcp/status                    Pool utilization and config
```

### IPAM

```
GET    /api/v1/ipam/pools                     List IP pools
GET    /api/v1/ipam/allocations               List allocations
POST   /api/v1/ipam/allocate                  Allocate IP
DELETE /api/v1/ipam/allocations/{id}          Release IP
```

### Logs

```
GET    /api/v1/logs?limit=100&level=info&module=dhcp
```

In-memory ring buffer (1000 entries) with level/module filtering.

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
| `http` | HTTP GET, expects 2xx | `:80/health` |
| `https` | HTTPS GET, expects 2xx | `:443/health` |
| `tcp` | TCP connect test | `:80` |

Unhealthy records are automatically excluded from DNS responses. The state machine uses configurable thresholds to prevent flapping.

## Scripts

| Script | Purpose |
|--------|---------|
| `scripts/build-and-push.sh` | Cross-compile ARM64, build stormdbase container, push to registry |
| `scripts/import-zones.sh` | Import zones from PowerDNS |
| `scripts/clone-dhcp-reservations.sh` | Clone DHCP reservations from MikroTik routers |
| `scripts/test-dhcp.sh` | DHCP testing utilities |

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
- Federation uses NATS for heartbeat and config sync between instances

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

- **Automatic cleanup** — Background task purges expired DHCP leases (4x lease time retention). Periodic sync rebuilds in-memory state every 60s.
- **Orphaned lease cleanup** — Scans for lease entries not referenced by MAC index and removes them.

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
  microdns-msg/        Message bus abstraction (NATS, NoOp)
  microdns-federation/ Leaf/coordinator agents, heartbeat, config sync
  microdns-api/        REST (axum) + gRPC (tonic) + dashboard + WebSocket
```

## License

MIT
