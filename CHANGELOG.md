# Changelog

## [Unreleased]

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
