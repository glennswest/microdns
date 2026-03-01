# Changelog

## [Unreleased]

### 2026-03-01
- **feat:** DHCPv4 dual mode — `normal` (direct broadcast, standard DHCP) and `gateway` (relay-only with veth deadman timer for RouterOS/Rose containers)
- **fix:** DHCPv4 now works on non-relay deployments (e.g. Proxmox LXC at 192.168.1.52) — previously all direct broadcasts were silently dropped
- **fix:** DHCP broadcast response — OFFER/ACK now broadcast when client has no IP (`ciaddr==0`) instead of unicasting to `yiaddr` (which fails because ARP can't resolve a client that doesn't have the IP yet)
- **fix:** DHCP pool exhaustion — added 60-second periodic `sync_pool()` that rebuilds the in-memory allocated IP set from active leases and reservations, preventing phantom allocations from expired leases that were never freed
- **fix:** DNS auto-registration deduplication — `register_v4()`/`register_v6()` now check for existing records before creating. If identical record exists (same name+type+data), skip. If hostname moved to new IP, remove stale records first. Applies to both A/AAAA and PTR records. Previously, every DHCP ACK created a new DNS record without checking, causing massive duplicate explosion (e.g. 56 copies of `konnected-b8959c`, 39 copies of `Samsung`)
- **fix:** DHCP lease upsert — `create_lease()` now checks for existing lease by MAC via the index. If found, updates the existing entry in place (new times, same UUID) instead of creating a new row that orphans the old one. Prevents `list_active_leases()` from returning duplicates
- **fix:** Orphaned lease cleanup — added `purge_orphaned_leases()` that finds LEASES_TABLE entries whose UUID is not referenced by the MAC index and deletes them. Runs every 60 seconds alongside the expired lease purge
- **fix:** Lease purge retention — changed from 24 hours to 2400 seconds (4x the 600s lease time). Purge interval reduced from 300s to 60s for faster cleanup
- **chore:** DNS zone cleanup — deleted 500+ duplicate records from gw.lo zone (blink cameras, DHCP-registered duplicates), zone reduced from 600+ to 160 clean records
- **chore:** Added `Dockerfile.cross-amd64` for cross-compiling x86_64-unknown-linux-musl from ARM64 macOS host using `gcc-x86-64-linux-gnu` (avoids QEMU SIGSEGV crashes)
- **chore:** CAP AP reservations updated: cap01→.254, cap02→.253, cap03→.252
- **chore:** Added phone reservation: 80:96:98:3C:10:12 → .95 (hostname ap01)

### 2026-02-28
- **feat:** x86_64-unknown-linux-musl cross-compile support (`.cargo/config.toml` linker config)
- **feat:** `deploy_mdns_gw.sh` — deploy script for mdns.gw.lo (192.168.1.52, Alpine/OpenRC)
- **feat:** Full DNS sync to mdns.gw.lo — all PowerDNS + MikroTik DHCP records synced, duplicates cleaned, reverse DNS rebuilt
- **feat:** `sync_mdns_gw.py` — comprehensive sync script (PowerDNS import, MikroTik DHCP, dedup, reverse DNS rebuild)
- **feat:** Forward zones for g10.lo, g11.lo, gt.lo on mdns.gw.lo (non-auth forwarding)
- **fix:** `dns.gw.lo` now points to 192.168.1.52 (mdns) instead of stale 192.168.1.154
- **fix:** MikroTik router upstream DNS changed from dnsx (192.168.1.51) to mdns (192.168.1.52)
- **fix:** Removed duplicate zones (s1.lo x2, g10.lo x2, g11.lo x2, reverse zones x2)
- **fix:** Cleaned 28 duplicate records in gw.lo, 27 in s1.lo

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
