# Changelog

## [0.9.0] - 2026-08-18

### Changed
- **feat(mdns):** One shared `mdns.lo` zone, held in one place. A single instance holds the zone; every other instance listens on its own segment and registers what it hears **into that instance** over the REST API, then points its own clients there for that zone. Replaces the per-instance copies of 0.8.0 and the per-network subzones (`mdns.g9.lo`) of 0.5.0: a device keeps the address it has on its own network, and its name is simply `something.mdns.lo` from everywhere. Set `holder` to the address of the instance holding the zone; leave it empty on that instance
- **feat(api):** `POST /zones/{id}/records` accepts an optional `source`, so a record registered over the API by an automatic source stays labelled as such — which is what lets a reporting instance withdraw its own names later without touching anyone else's
- **removed:** the peer-mirroring machinery (`peers`, `peer_sync_secs`) added in 0.8.0. There is one copy of the zone now, so there is nothing to mirror

## [0.8.0] - 2026-08-18

### Changed
- **feat(mdns):** Discovered names now land in **one flat domain shared by every instance** (`mdns.lo` by default) instead of a per-network subzone. Each instance publishes what it hears on its own segment and mirrors what its siblings hear, so a name resolves identically from any network and callers no longer need to know which subnet a device sits on. Mirroring is a pull (`GET /api/v1/mdns/discovered` every `peer_sync_secs`, default 30 s) and a sibling only ever reports what it heard itself, which is what stops instances echoing copies back and forth. Siblings are derived from the DNS forwarders an instance already has, so a new network needs no mDNS configuration; `peers` overrides that, and `peer_sync_secs = 0` restores per-instance behaviour. A sibling that cannot be reached keeps its last reported names, which then age out on their own TTL rather than vanishing because one poll failed
- **feat(api):** `/api/v1/mdns/status` now separates `heard_here` from `mirrored` and lists each sibling with its name count and last sync; `/api/v1/mdns/discovered` reports the answering `instance_id` and — deliberately — only what that instance heard itself

## [0.7.0] - 2026-08-18

### Added
- **feat(auth):** Zone-transfer settings are now stored in the database and managed through the API — `GET`/`PUT`/`DELETE /api/v1/zone-transfer/config` covering `allow_transfer`, `notify` and `secondary`. All three consumers read the same live settings, so an AXFR ACL change, a new NOTIFY target or a newly mirrored zone takes effect within ten seconds without a restart: the listener re-reads the ACL per request, the announcer re-reads its targets per batch, and the mirror agent re-reads its zone list per tick. The `[dns.auth]` fields seed the stored value on first run only. Same reason as the mDNS section — on mkube-managed instances the TOML is regenerated from a Network CRD and hand-added blocks do not survive

### Changed
- **refactor(auth):** `Notifier` is now a stateless `notify_zone(zone, targets)`, since the target list is no longer fixed at startup

## [0.6.1] - 2026-08-18

### Fixed
- **fix(dns):** A subzone's names are now resolved from the subzone rather than from whichever suffix-matching zone happened to come first. `query_fqdn` and `fqdn_exists` iterated zones and answered from the first match, so with both `g9.lo` and `mdns.g9.lo` present, a query for `teslatracker-52c4.mdns.g9.lo` could be answered from `g9.lo` — which holds nothing under that name — and return empty. Both now select the most specific (longest) matching zone, the way `find_zone_for_fqdn` already did. Found while verifying the mDNS rollout on g9: the records existed and neither the auth server nor the recursor would serve them

## [0.6.0] - 2026-08-18

### Added
- **feat(auth):** Secondary zones — `[[dns.auth.secondary]]` mirrors a zone from a primary over AXFR, driven by inbound NOTIFY and by a per-zone refresh timer. A check asks the primary for the zone's SOA and transfers only when the serial differs (RFC 1982 arithmetic, so wrapped serials compare correctly); an unreachable primary is a warning, not an error, and the existing copy keeps being served. Inbound NOTIFY (RFC 1996) is answered on UDP and TCP, and believed only from that zone's configured primary — a NOTIFY is an instruction to go and transfer a zone, so accepting one from anywhere would let any host aim this instance's transfers. Documented in `docs/zone-transfer.md`
- **feat(auth):** Outbound NOTIFY is now wired to every write. `Db::increment_soa_serial` is the one call every writer makes to finish a change, so a zone-change hook there covers the REST API, DHCP registration, the mDNS and Kubernetes sources and reverse-PTR sync alike. Changes are collapsed over a two-second window to one NOTIFY per zone, since a single edit can bump a serial two or three times
- **feat(mdns):** mDNS ingest configuration moved into the database (`runtime_config` section `mdns`) with `GET`/`PUT`/`DELETE /api/v1/mdns/config`. The source follows the stored config live — starting, stopping and re-homing its zone without a restart, and withdrawing what it published when turned off. A `[mdns]` block in the config file now only seeds the stored value the first time. This is what makes the feature usable on mkube-managed instances, where `microdns.toml` is regenerated from a Network CRD and a hand-added block does not survive

### Changed
- **refactor(auth):** An inbound AXFR now upserts the zone and replaces its records in one pass instead of deleting and rebuilding it. The old path left a window in which the server answered "no such zone" for a zone it held a good copy of — on a secondary, exactly while the primary was being changed. `TransferResult` also carries the transferred `serial`

## [0.5.0] - 2026-08-18

### Added
- **feat(mdns):** mDNS ingest (`microdns-mdns`) — listens for `.local` announcements on the instance's own segment and publishes them as ordinary authoritative records, so cross-subnet clients can resolve names that multicast (IP TTL 1) can never reach them (closes #8). Passive listening plus periodic DNS-SD browsing (`_services._dns-sd._udp.local`, then each service type learned); addresses, PTR/SRV/TXT service records, goodbye (TTL 0) withdrawal, TTL clamping into a configured window, and RFC 6762 §5.2 cache maintenance that re-queries at 80% of a record's lifetime so a device that only ever announces at boot stays published. Names inside rdata are rewritten into the publish zone, so a DNS-SD browse that starts in the zone stays there. Off by default; enable with `[mdns] enabled = true` and a `zone`. Documented in `docs/mdns-ingest.md`
- **feat(core):** Records now record their origin — new `RecordSource` (`manual` / `dhcp` / `mdns` / `k8s`) on `Record`, surfaced as `source` in the REST record JSON. Automatic sources own what they create and may prune it; `manual` records are never touched by a source and win any conflict with a discovered name. Rows written before the field existed deserialize as `manual`, which is the safe reading. DHCP auto-registration and the Kubernetes source now label their records (and the PTRs they sync) accordingly
- **feat(api):** `GET /api/v1/mdns/status` (counters, cache size, service types seen) and `GET /api/v1/mdns/discovered` (the live discovery table, including names config filtered out and where each is published — `published_as: null` marks a filtered name, which is what separates "not announcing" from "not published")
- **feat(auth):** AXFR is now access-controlled and chunked. `[dns.auth] allow_transfer` takes a list of CIDRs (default: RFC1918 + loopback) and any TCP peer outside them gets REFUSED before a single record is read — an AXFR hands over a complete map of internal hosts. Transfers are also split into messages of 100 records (RFC 5936 §2.2) instead of one giant message, so a large zone can no longer overflow the 16-bit TCP length prefix and silently corrupt the transfer
- **feat(auth):** Outbound DNS NOTIFY sender (`microdns-auth::notify::Notifier`, RFC 1996) with `[dns.auth] notify` config listing secondaries as `ip` or `ip:port`. Collapses a secondary's change-detection latency from the 3600 s SOA refresh to seconds. The module and config are in place; wiring it to the record-write path is still outstanding
- **feat(api):** `/api/v1/health` now reflects database readability (enhancement request from mkube, `enhancements/health-reflects-database.md`). 200 is returned only when a cheap zone-count read succeeds (new `Db::zone_count()` using redb table `len()` — no deserialization; zero zones is healthy). A missing/locked/corrupt database returns 503 with `{"status":"unhealthy","check":"database","error":"..."}`. mkube v6.2.1 gates all DNS operations per instance on this probe (15s TTL, 1.5s timeout)

## [0.4.0] - 2026-07-14

### Changed
- **feat(msg):** NATS messaging is now **opt-in** behind the `nats` cargo feature and **off by default**. Default builds exclude `async-nats` entirely. In a Kubernetes deployment the kube-apiserver watch (etcd) handles change propagation, so NATS is not needed; enable with `--features nats` for the standalone federated LAN topology. A `nats` backend configured in a build without the feature falls back to noop with a clear error.

## [0.3.0] - 2026-07-14

### Added
- **feat(k8s):** Kubernetes DNS source (`microdns-k8s`) — watches a kube-apiserver and serves a cluster's internal zone (`cluster.local` + reverse zones) live from Services, EndpointSlices/Endpoints and Pods, implementing the Kubernetes DNS-Based Service Discovery spec (CoreDNS / OpenShift-DNS equivalent). ClusterIP (dual-stack A/AAAA), headless (per-endpoint records honoring `ready`/`publishNotReadyAddresses`), ExternalName (CNAME), SRV for named ports, pod records, apex `NS`/`ns.dns`/`dns-version` TXT, PTR with stale-IP pruning. Full IPv6 parity. Runs in-process (like CoreDNS's kubernetes plugin), auto-enables when running inside a Kubernetes pod.
- **build:** deb/rpm packaging (nfpm) + native systemd unit for arm64/armv7, and a size-optimized release profile (strip + LTO + `panic=abort`).

## [Unreleased]

### 2026-05-03
- **feat:** Persisted "last queried" timestamp per `(fqdn, type)` — every DNS query landing on the auth server bumps an in-memory `QueryTracker` (lock-free `DashMap`); a periodic 60 s flush task writes dirty rows to a new `query_stats` redb table. Hydrated on startup so the dashboard view survives a quick restart. Surfaced in `/api/v1/lb/resolutions` (`last_queried_at`, `query_count`) and rendered in the Resolution panel under each FQDN ("Last queried 2m ago · 4,182 total"). Also useful operationally to spot stale records that nothing actually resolves

### 2026-05-01
- **docs:** Load balancer parity design — `docs/loadbalancer-design.md` inventories ploadb (pdnsloadbalancer) functionality, gaps in current `microdns-lb`, and approved plan to reach feature parity (per-record probe config, two-pass parallel cycle, real ICMP, last-alive failsafe, REST API, dashboard wiring, persisted health state in new `lb_record_health` redb table with staleness indicator)
- **fix:** DHCP DNS registration — reservation hostname now takes priority over the client's announced hostname (was the other way around). Ensures clients with stable reservations register the operator-assigned name in DNS regardless of what the device claims to be called
- **feat:** LB persisted health storage — new `lb_record_health` redb table keyed by `record_id` with `PersistedHealth` rows (status, timestamps, counters, last probe detail). Methods on `Db`: `list_lb_health`, `get_lb_health`, `upsert_lb_health_batch`, `delete_lb_health`. Health rows are auto-removed when the underlying record is deleted. New `HealthStatus` enum (Unknown/Healthy/Unhealthy)
- **feat:** LB monitor refactored to two-pass parallel probe cycle — collects all health-checked records, probes them concurrently (capped by `probe_concurrency`), then applies state transitions + last-alive failsafe in a single decision pass. Eliminates the flip-flop window between failsafe activation and the next probe. Per-record state tracks `last_checked_at`, `last_state_change_at`, `last_healthy_at`, `last_probe_detail`, `last_probe_type`. Cycle ends with one batched redb txn persisting the snapshot
- **feat:** LB hydration on startup — restores `HealthState` from `lb_record_health`; orphan rows (record deleted while microdns was down) are pruned. After restart, dashboards see the previous health view immediately
- **feat:** LB last-alive failsafe — when every member of a `(zone, name, type)` group is `Unhealthy`, the member with the most recent `last_healthy_at` is force-enabled (deterministic, replaces previous HashMap-order pick). Failsafe state-change events are emitted with `failsafe=true`
- **feat:** Real ICMP ping via `surge-ping` — uses raw ICMP sockets when `CAP_NET_RAW` is available; logs a single warning and falls back to the TCP-reachability stand-in when not. Configurable `ping_packet_count` (default 3, matches ploadb)
- **feat:** New `MonitorConfig` with `probe_concurrency`, `ping_packet_count`, `default_timeout_secs` knobs in `[dns.loadbalancer]`. New `LbHandles` exposes monitor state + state-change broadcast to the API server (`ApiServer::with_lb`)
- **feat:** LB REST API at `/api/v1/lb/*`:
  - `GET /lb/status` — overall counts + ICMP availability + last-cycle window
  - `GET /lb/groups` — per `(zone, name, type)` summary with member/healthy counts and failsafe flag
  - `GET /lb/records` — per-record health rows including `last_checked_at`, `stale` flag, and `age_seconds`
  - `PUT /zones/{zone_id}/records/lb/{name}/{type}` — bulk apply a `HealthCheck` blob to every member of a name
  - `DELETE /zones/{zone_id}/records/lb/{name}/{type}` — bulk clear `HealthCheck` from every member
  - `POST /lb/probe/{record_id}` — fire a one-shot probe and return the result (ops/debug)
- **feat:** Dashboard "Load Balancer" tab populated — pulls from `/lb/{status,groups,records}`. Columns: zone / name / IP / status badge / probe / last-check (with age & stale flag) / probe detail. Failover groups card shows multi-member groups with per-IP dots and a FAILSAFE badge when the group is all-down. ICMP-unavailable banner shown when raw sockets aren't usable
- **feat:** Dashboard WebSocket emits `LbStateChange` events (status flip / failsafe activation). Events appear in the Events tab and trigger an immediate LB-tab refresh when visible. New `lb` category added to the SSE `/watch` filter
- **feat:** New `ProbeType::TcpHalfOpen` — long-running keepalive monitor. Each `tcp_half_open` record gets one persistent TCP connection (`SO_KEEPALIVE` + `TCP_KEEPIDLE/INTVL/CNT` tuned from `hc.timeout_secs`). No application data flows; failure detection is event-driven via the kernel keepalive (no probe duty cycle). When the connection breaks, the record flips to Unhealthy immediately and a reconnect loop kicks in. Implemented in `microdns-lb/src/halfopen.rs` (`HalfOpenManager`). `ProbeType` JSON now uses `snake_case`; existing single-word variants unchanged
- **feat:** State-change ring buffer — last 200 transitions kept in memory with old/new status, surfaced via `GET /api/v1/lb/log`
- **feat:** Live-resolution endpoint — `GET /api/v1/lb/resolutions` returns, per `(zone, name, type)`, the IPs the authoritative server would currently return (members with `enabled=true`), with a `failsafe` flag on entries that are only included because the group is all-down
- **feat:** Debug endpoint — `GET /api/v1/lb/debug` raw dump of in-memory `HealthState` + persisted rows, ICMP availability, and half-open watcher count. Equivalent of ploadb's `/debug`
- **feat:** Dashboard LB tab redesigned to match ploadb's three real-time panels: **Load Balanced Targets** (tree of zones → hostnames → IPs with status, probe type, and "in-this-state-for Xh ↑/↓" annotation), **Current DNS Resolution** (per-FQDN service status (`UP for 3h` / `DOWN since 5m ago`), live answers with TTL per IP and failsafe markers), **Status Change Log** (Time / Hostname / IP / Change / Probe table — same columns as ploadb). Stat cards stay at the top; ICMP-unavailable banner above the panels
- **feat:** Per-FQDN service uptime — `/api/v1/lb/resolutions` now reports `service_status` (up / down / unknown) and `service_since` per group. Computed from member health: UP if any member is Healthy with `service_since` = the longest continuously-Healthy member's transition timestamp; DOWN with `service_since` = the most recent member transition (when the last one fell over)
- **chore:** k8s manifest — `NET_BIND_SERVICE` + `NET_RAW` capabilities now also set on `deployment-coordinator.yaml` (leaf already had them). Production deploy through mkube tracked in [mkube#12](https://github.com/glennswest/mkube/issues/12)

### 2026-04-27
- **feat:** Add uptime to health check API — `/api/v1/health` now returns `uptime_seconds` (u64) and `uptime` (human-readable string e.g. "3d 2h 15m 42s")
- **fix:** Default config DNS listen port 15353 → 53 — baked-in config caused stormd liveness probe (TCP 53) to fail when deploy ConfigMap wasn't mounted, triggering 30s restart loop

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
