# Changelog

## [Unreleased]

### 2026-04-27
- **feat:** Add uptime to health check API — `/api/v1/health` now returns `uptime_seconds` (u64) and `uptime` (human-readable string e.g. "3d 2h 15m 42s")

### 2026-04-01
- **fix:** Add DHCP debug logging for root_path/iPXE diagnostics — logs reservation lookup, root_path chain (reservation → pool → effective), iPXE detection, and boot file selection per MAC

### 2026-03-28
- **feat:** Automatic reverse zone generation and PTR sync — A/AAAA records created, updated, or deleted via REST API now auto-create reverse zones (in-addr.arpa / ip6.arpa) and maintain corresponding PTR records
- **feat:** DHCP DNS registrar auto-creates reverse zones instead of silently skipping when reverse zone doesn't exist
- **refactor:** New `microdns_core::reverse` module with reusable reverse DNS utilities (zone name computation, PTR sync/delete, ensure_reverse_zone)

### 2026-03-29
- **feat:** New gw network microdns instance at 192.168.1.252 — replaces .52, aligns with .252 convention used by g10/g11/g8/g9
- **feat:** Added `domain_search` (option 119) to gw DHCP pool for cross-network hostname resolution
- **fix:** Removed stale cap03 reservation at .252 (belongs on g9 network)
- **fix:** Updated all peer configs (g10, g11, gt) to forward gw.lo to 192.168.1.252 instead of .52
- **chore:** Removed pv.lo, bm.lo, ipmi.lo zones — deleted from forward zones and domain_search across all configs
- **feat:** Bootstrap script for gw252 — transfers zones from .52, creates reverse zone, pre-populates A+PTR for all 47 DHCP reservations, cleans up junk DNS records (phones, cameras, cars, auto-DHCP names, duplicates)

### 2026-03-20
- **fix:** Add comprehensive DNS forwarding across all networks — each instance now forwards to all other networks (g8, g9, g10, g11, gt, gw) including reverse zones (in-addr.arpa) and utility zones (pv.lo, bm.lo, ipmi.lo)
- **fix:** Corrected stale DNS forwarder IPs in gt config (192.168.10.199 → 192.168.10.252, 192.168.11.199 → 192.168.11.252)
- **feat:** DHCP option 119 (domain search) includes all `.lo` zones so systemd-resolved routes cross-network queries to local microdns — fixes "Name not found" for e.g. `registry.gt.lo` on g10 clients
- **feat:** `domain_search` field added to TOML pool config (`DhcpV4Pool`) and wired through TOML-to-DB migration
- **fix:** Suppress DHCP option 15 (domain name) when option 119 (domain search) is configured — option 15 causes systemd-resolved to scope the DNS server to a single domain, breaking cross-network resolution
- **fix:** Bounded shutdown timeout (8s) prevents container restart loops — axum graceful shutdown was waiting indefinitely for long-lived WebSocket/SSE connections to close
- **fix:** Return NOERROR (not NXDOMAIN) for queries where the name exists but has no records of the queried type — fixes systemd-resolved parallel A+AAAA lookups where NXDOMAIN on AAAA was poisoning results for names that only have A records
- **feat:** Switch container base from `scratch` to `stormdbase` — adds process supervision, SSH access, web dashboard, liveness probes, structured logging, and busybox commands

### 2026-03-18
- **fix:** DHCP reservations now inherit all extended options (NTP, MTU, domain search, log server, time offset, WPAD) from pool when not explicitly overridden — previously these options were only emitted when set directly on the reservation

### 2026-03-16
- **feat:** REST API for DHCP pool static routes: `GET/POST /api/v1/dhcp/pools/{id}/routes`, `DELETE /api/v1/dhcp/pools/{id}/routes/{route_id}`
- **feat:** DHCP option 121 (RFC 3442) emitted from pool-level static routes, with automatic default route (`0.0.0.0/0 via gateway`) injection
- **feat:** `StaticRoute` now has `id` (UUID) and `managed_by` fields for route ownership tracking (e.g. CloudID)
- **feat:** Duplicate route detection (same destination+gateway returns existing with 200 OK)
- **fix:** Pool-level static routes now served to all clients in a pool, not just per-reservation

### 2026-03-06
- **fix:** DHCP pool allocator loads from DB, not TOML — root cause of "no available IPs" when mkube pushes pools via REST API
- **fix:** Removed `from_db()` constructor (redundant with `new()` which now loads from DB)
- **fix:** `get_reservation()` reads DB only, removed TOML config fallback
- **fix:** `sync_pool()` rebuilds full pool list from DB every 60s (picks up pools added via REST after boot)
- **fix:** `/dhcp/status` endpoint reads pools and reservations from DB, not TOML config
- **refactor:** Removed `reservations` HashMap field and all TOML pool/reservation loading from DHCP server

### 2026-03-05
- **feat:** Database-driven DHCP/DNS config — all pools, reservations, forwarders stored in redb, CRUD via REST API
- **feat:** New redb tables: `dhcp_pools`, `dhcp_reservations`, `dns_forwarders`, `instance_config` with full CRUD
- **feat:** REST API: POST/GET/PATCH/DELETE for `/dhcp/pools`, `/dhcp/reservations`, `/dhcp/config`, `/dns/forwarders`
- **feat:** Extended DHCP options: NTP servers (opt 42), MTU (opt 26), domain search (opt 119), classless static routes (opt 121), log server (opt 7), time offset (opt 2), WPAD (opt 252)
- **feat:** DHCP server reads pools/reservations directly from database (no in-memory cache, no reload signals)
- **feat:** Recursor reads forward zones directly from database on each query
- **feat:** CLI bootstrap: `--listen-dns`, `--data-dir`, `--nats-url`, `--mode`, `--dhcp-interface`, `--instance-id` flags
- **feat:** TOML→database one-time migration on first boot (backward compat)
- **refactor:** Removed all reload channels — redb is memory-mapped, reads are free
- **feat:** Dashboard rewrite — 7-tab SPA (Overview, DNS, LB, DHCP, Events, Logs, Peers)
- **feat:** DHCP tab: full CRUD for pools and reservations with all extended option fields
- **feat:** Events tab: real-time event stream from broadcast channel with type filtering
- **feat:** WebSocket: two message types (snapshot + event) via tokio::select!
- **feat:** SSE watch endpoint: `GET /api/v1/watch?types=dhcp,dns,zones,records,leases`
- **feat:** Zone/record event publishing to DashboardEvent broadcast + NATS MessageBus
- **feat:** NATS publishing from all mutation handlers (pools, reservations, forwarders, zones, records)

### 2026-03-01
- **feat:** DHCPv4 dual mode — `normal` (direct broadcast, standard DHCP) and `gateway` (relay-only with veth deadman timer for containerized deployments)
- **fix:** DHCPv4 now works on non-relay deployments — previously all direct broadcasts were silently dropped
- **fix:** DHCP broadcast response — OFFER/ACK now broadcast when client has no IP (`ciaddr==0`) instead of unicasting to `yiaddr` (which fails because ARP can't resolve a client that doesn't have the IP yet)
- **fix:** DHCP pool exhaustion — added 60-second periodic `sync_pool()` that rebuilds the in-memory allocated IP set from active leases and reservations, preventing phantom allocations from expired leases that were never freed
- **fix:** DNS auto-registration deduplication — `register_v4()`/`register_v6()` now query existing records before creating. If an identical record exists (same name+type+data), skip creation entirely. If hostname moved to a new IP, remove stale records first. Applies to both forward (A/AAAA) and reverse (PTR) records. Previously, every DHCP ACK blindly created a new DNS record, causing unbounded duplicate growth
- **fix:** DHCP lease upsert — `create_lease()` now looks up existing lease by MAC via the index. If found, updates the existing entry in place (new timestamps, same UUID) instead of inserting a new row that orphans the old one. Prevents `list_active_leases()` from returning duplicate entries per client
- **fix:** Orphaned lease cleanup — added `purge_orphaned_leases()` that scans the lease table for entries whose UUID is not referenced by the MAC index and removes them. Runs every 60 seconds to clean up any leftover state
- **fix:** Lease purge retention — expired leases now kept for 4x the lease time before reaping (was 24 hours). Purge interval reduced from 300s to 60s for faster cleanup of stale entries
- **chore:** Added `Dockerfile.cross-amd64` for cross-compiling x86_64-unknown-linux-musl from ARM64 host using `gcc-x86-64-linux-gnu` (avoids QEMU emulation crashes)
- **chore:** Updated DHCP static reservations for CAP access points and additional devices

### 2026-02-28
- **feat:** x86_64-unknown-linux-musl cross-compile support (`.cargo/config.toml` linker config)
- **feat:** Deploy script for Alpine/OpenRC target hosts
- **feat:** Full DNS zone sync — PowerDNS + DHCP records imported, duplicates cleaned, reverse DNS rebuilt
- **feat:** Sync script for comprehensive zone migration (PowerDNS import, DHCP hostname import, dedup, reverse DNS rebuild)
- **feat:** Forward zone delegation for multi-network DNS resolution
- **fix:** Corrected upstream DNS references to point to active microdns instance
- **fix:** Removed duplicate zones created during migration
- **fix:** Cleaned duplicate records across multiple zones

### 2026-02-27
- **feat:** Full management dashboard — 5-tab SPA (Overview, DNS, DHCP, Logs, Peers)
- **feat:** DNS CRUD — create/delete zones, create/edit/delete records (all 9 types) with inline editing
- **feat:** DHCP tab — pool config, active leases with search/filter
- **feat:** Logs tab — filtered log viewer with level/module dropdowns and auto-refresh
- **feat:** Peers tab — connectivity probe cards with latency display
- **feat:** CORS on API router — allows dashboard on :80 to fetch from API on :8080
- **feat:** Skip API key for GET requests — read-only access without authentication
- **feat:** Load Balancer tab — aggregates health-checked records across all zones, shows healthy/unhealthy counts, failover groups with failsafe detection

### 2026-02-26
- **feat:** Split REST API and dashboard onto separate ports — API on :8080, dashboard on :80
- **feat:** Add `dashboard_listen` config option to `[api.rest]` section
- **feat:** Add `/` → `/dashboard` redirect on dashboard port

### 2026-02-24
- **chore:** Add build.sh/deploy.sh for podman scratch container build + push to local registry (matches ipmiserial pattern)
- **fix:** Dedup DNS record creation — when creating a record with identical name, type, and data to an existing record, return the existing record (HTTP 200) instead of creating a duplicate. Prevents accumulation of duplicate entries from repeated mkube reconcile cycles.

### 2026-02-23
- **feat:** Add in-memory log ring buffer (1000 entries) with REST endpoint `GET /api/v1/logs?limit=100&level=info&module=dhcp`
- **feat:** Custom tracing Layer captures all log events into queryable ring buffer
- **fix:** Add 30s DHCP recv deadman timer — auto-recycles socket when stuck (veth corruption recovery)
- **fix:** Replace fatal `bind_recv_socket` crash with 5s retry loop for transient bind failures
- **fix:** Elevate DHCP activity logs (Discover/Offer/Request/ACK) from debug to info level
- **fix:** Veth corruption workaround — per-transaction bind/send/drop socket pattern
- **feat:** iPXE client detection (option 175 + user-class) with HTTP boot URL support
- **feat:** Configurable `server_ip` for siaddr/option 54 (prevents DHCP relay confusion)
- **fix:** Force broadcast flag on relay responses for proper client delivery
- **fix:** Handle SIGTERM in addition to SIGINT for container lifecycle
- **fix:** Skip raw broadcast DHCP packets (giaddr=0) — only process relay unicasts
