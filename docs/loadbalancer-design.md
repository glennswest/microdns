# MicroDNS Load Balancer — Design (Parity with pdnsloadbalancer)

Status: **APPROVED — ready for implementation**
Date: 2026-05-01

## Approved decisions (2026-05-01)

- **Per-record config** (decided): probe configuration stays on each
  `Record.health_check`. No new `lb_groups` table, no inheritance. The REST
  API gets a convenience endpoint to apply a `HealthCheck` blob to every
  member of a `(zone, name, type)` group in a single call, but the storage
  remains per-record. Grouping for failsafe is still computed at runtime
  from `(zone_id, name, record_type)` (already implemented in `state.rs`).
- **Real ICMP via `surge-ping`**: yes. DaemonSet gets `NET_RAW`. TCP fallback
  with one-time warning if the capability isn't available.
- **No persistence of runtime health state**: confirmed. Records start
  `Unknown`, stay enabled, get reprobed within `check_interval_secs` after
  startup.
- **One umbrella PR / one umbrella issue** (not 8 separate ones).
- **LB dashboard**: build it (populate the existing tab shell + WS
  `lb_state_change` events).

## 1. Why this exists

`microdns-lb` was stubbed during Phase 3 but never reached feature parity with the
existing `pdnsloadbalancer` (ploadb) that microdns is supposed to replace. Today
the crate compiles, runs in main.rs, and has a probe loop — but it is a partial
re-skin, not a port. This document inventories what ploadb does, what microdns
already has, and what remains.

## 2. ploadb feature inventory

Source: `/Volumes/minihome/gwest/projects/pdnsloadbalancer/`

### 2.1 Core behavior
- Polls PowerDNS API every **20 s** for all zones.
- For every A rrset with **2+ records**, treats the rrset as a load-balanced group.
- Probes each IP with the per-rrset probe config; flips the `disabled` bit on each
  PowerDNS record to remove unhealthy IPs from query responses.
- **Two-pass cycle** (v2.2 fix): probe everything, *then* decide. Prevents
  flip-flop where failsafe enables a host and the next cycle disables it again.
- **Failsafe** (v2.1): if all members of a group are down, keep the **last
  alive** (most recently enabled) host enabled so DNS never returns NODATA.

### 2.2 Probe types
- **ICMP ping** — default. 3 packets, healthy if any reply. Needs `cap_net_raw`.
- **HTTP(S)** — GET path, configurable port/timeout/expected-status. HTTPS skips
  cert validation.
- **TCP** — connect-only check (added later, used in production for
  `api.gw.lo:6443` and `*.apps.gw.lo:80`).

### 2.3 Probe config storage
Per-rrset JSON blob stuffed into the **PowerDNS record `comments` field**:
```json
{"type":"tcp","port":6443,"timeout":10}
{"type":"http","path":"/health","port":8080,"timeout":10,"expected":200}
{"type":"https","path":"/api/status","port":443,"timeout":5,"expected":200}
{"type":"ping","timeout":5}
```
Defaults: `type=ping`, `path=/`, `port=80/443`, `timeout=5`, `expected=200`.
Parse failure or no comment ⇒ ping.

### 2.4 GUI
Built-in HTTP server (default port `8080`) with a WebSocket-driven dashboard:
zones grouped, hostnames grouped under zones, IP rows with ENABLED/DISABLED
badges, last-check time, probe type. Read-only.

### 2.5 Logging
Lumberjack-rotated file (`/var/log/ploadb/ploadb.log`, 5 MB × 3 backups, 28 d).
State-change lines like `api.gw.lo. - 192.168.1.10 changed state to false (tcp)
(failsafe)`.

## 3. What microdns-lb already has

| Capability                          | Status | Notes                                                           |
|-------------------------------------|--------|-----------------------------------------------------------------|
| Periodic probe loop                 | ✅      | `monitor.rs`, configurable `check_interval_secs`                |
| HTTP / HTTPS probe                  | ✅      | `reqwest`, accepts invalid certs                                |
| TCP probe                           | ✅      | `tokio::net::TcpStream` + timeout                               |
| ICMP ping probe                     | ❌      | Currently fakes ICMP via TCP/80→TCP/443 fallback                |
| Per-record `HealthCheck` config     | ✅      | `Record.health_check: Option<HealthCheck>` on every record      |
| Healthy/unhealthy thresholds        | ✅      | Better than ploadb (which is binary single-shot)                |
| Disabled-record filtering at query  | ✅      | `db.rs:310` — disabled records dropped from rrset response      |
| Failsafe                            | ⚠️     | Implemented but picks **first by HashMap iter** (nondeterm.)    |
| Two-pass probe cycle                | ❌      | Probes serially and mutates between probes — flip-flop risk     |
| Probe parallelism within a cycle    | ❌      | Sequential `for record in ...`                                  |
| LB dashboard tab                    | ⚠️     | UI shell exists in `dashboard.rs` but no backing endpoint/data  |
| REST endpoint for LB state          | ❌      | No `/api/v1/lb/*`                                               |
| WebSocket live updates              | ❌      |                                                                 |
| Last-checked timestamp per record   | ❌      | `RecordHealth` doesn't track `last_checked`                     |
| Probe-cycle metrics / counters      | ❌      |                                                                 |
| Initial state = "unknown"           | ❌      | Starts optimistically `healthy = true`                          |

## 4. Modeling decisions

### 4.1 Record vs rrset

ploadb operates on rrsets (one PowerDNS rrset = many `content` lines), microdns
stores **one DB row per A IP**. The grouping for load-balancing/failsafe is
already keyed on `(zone_id, name, record_type)` in `state.rs:121` — that is the
right primitive. Keep this model. No schema change.

### 4.2 Where probe config lives — per-record (decided)

Each A row carries its own `Record.health_check: Option<HealthCheck>`. No new
table, no inheritance.

Setting the same config on every IP of a load-balanced name is a write-side
ergonomic problem, not a runtime one. Solve it with a convenience endpoint:

```
PUT /api/v1/zones/{zone_id}/records/lb/{name}/{type}
  body: HealthCheck JSON
  → writes that HealthCheck onto every existing record matching
    (zone_id, name, type). New records added later still need the config
    set explicitly (or via the same endpoint re-run).
```

Failsafe grouping is already computed at runtime from
`(zone_id, name, record_type)` in `state.rs:121` — keep that.

### 4.3 Failsafe — last alive

Track `last_healthy_at: Instant` per record. When all members of a group go
unhealthy, force-enable the one with the most recent `last_healthy_at`. Matches
ploadb v2.2.

### 4.4 Two-pass cycle

Refactor `run_check_cycle` to:
1. **Collect** — build a list of `(record_id, group_key, probe_fn)` for all
   health-checked records.
2. **Probe in parallel** — `futures::stream::iter(...).buffer_unordered(N)`,
   capping concurrency (e.g. 32). Collect results.
3. **Decide per group** — apply threshold transitions, then evaluate failsafe
   on the post-decision view of the group. One `db.update_record` call per
   actually-changed record.

This kills the flip-flop window and lets us stop probing groups once we've
proven all-down for the cycle (cheap optimization).

### 4.5 Real ICMP

Use the `surge-ping` crate (or `ping-rs`). Linux container needs `cap_net_raw`
or `net.ipv4.ping_group_range = 0 2147483647` for unprivileged ICMP sockets.
stormdbase already runs as root in the container, so `cap_net_raw` via the
manifest's `securityContext.capabilities.add: [NET_RAW]` is the cleanest.
Fallback to current TCP-probe behavior if the socket can't be opened (log a
warning once at startup).

### 4.6 Initial state

New records register as `Unknown` not `Healthy`. First probe transitions to
Healthy or Unhealthy. Until the first probe completes, the record is left
**enabled** (matching ploadb behavior — disabled bit isn't flipped on startup).

## 5. API surface

### 5.1 New REST endpoints (`crates/microdns-api`)

```
GET  /api/v1/lb/status
       → { groups: [...], records: [...], last_cycle_at, cycle_duration_ms }

GET  /api/v1/lb/groups
       → list of (zone, name, type, member_count, healthy_count, probe_config,
                  failsafe_active)

GET  /api/v1/lb/records
       → list of (record_id, zone, name, ip, healthy, enabled,
                  last_checked_at, last_state_change_at, last_probe_detail,
                  consecutive_successes, consecutive_failures)

PUT  /api/v1/zones/{zone_id}/records/lb/{name}/{type}
       body: HealthCheck JSON
       → writes that HealthCheck onto every existing record matching
         (zone_id, name, type). Convenience for setting the same probe
         config on every IP of a load-balanced name.

DELETE /api/v1/zones/{zone_id}/records/lb/{name}/{type}
       → clears HealthCheck (sets to None) on every member record

POST /api/v1/lb/probe/{record_id}
       → fire a one-shot probe and return the result (for ops/debugging)
```

### 5.2 WebSocket

Reuse the existing dashboard WebSocket. Emit `lb_state_change` events:
```json
{
  "event": "lb_state_change",
  "record_id": "...",
  "zone": "gw.lo",
  "name": "api",
  "ip": "192.168.1.201",
  "healthy": false,
  "failsafe": false,
  "probe_type": "tcp",
  "detail": "tcp/6443: timeout",
  "at": "2026-05-01T12:34:56Z"
}
```

### 5.3 Dashboard tab

The "Load Balancer" tab shell already exists in `dashboard.rs` but is empty.
Wire it to `/api/v1/lb/status` + the WS event above. UI columns:
zone / name / IP / status badge / probe type / last-check / detail.

## 6. Storage

No schema changes. Probe config stays in `Record.health_check: Option<HealthCheck>`
on each row, exactly as today. `HealthState` stays in-memory: after restart,
every record starts `Unknown` and is reprobed within `check_interval_secs`.
The durable artifact is the `enabled` bit on the record, which is what query
responses already filter on.

## 7. Config (`config.toml`)

Extend `DnsLbConfig`:
```toml
[dns.loadbalancer]
enabled = true
check_interval_secs = 20      # ploadb default
default_probe = "ping"
probe_concurrency = 32        # max in-flight probes per cycle
ping_packet_count = 3
ping_packet_interval_ms = 200
default_timeout_secs = 5
```

## 8. Migration / rollout

1. **No breaking changes** — existing per-record `HealthCheck` keeps working.
2. ploadb instances can run alongside microdns during cutover (different
   targets — ploadb hits PowerDNS, microdns owns its own zones).
3. For each zone migrated from PowerDNS to microdns, an optional one-shot
   importer can read ploadb's rrset comments and write the equivalent
   `HealthCheck` onto every member record (out of scope for the umbrella PR
   — separate ticket).

## 9. Work breakdown (single PR)

The umbrella PR delivers all of the below; logically sequenced for reviewer
sanity but landed together:

1. **Two-pass cycle + parallel probes** — refactor `monitor.rs`. Add
   `last_checked_at`, `last_state_change_at`, `last_probe_detail`,
   `last_healthy_at` to `RecordHealth`. Initial state = `Unknown`.
2. **Last-alive failsafe** — pick member with most recent `last_healthy_at`
   (deterministic, matches ploadb v2.2).
3. **Real ICMP** — add `surge-ping` to `microdns-lb`. Detect missing
   `CAP_NET_RAW` at startup, log a one-time warning, fall back to current
   TCP-reachability stand-in.
4. **REST endpoints** — `/api/v1/lb/{status,groups,records}`,
   `PUT/DELETE /api/v1/zones/{zone_id}/records/lb/{name}/{type}` (bulk
   apply/clear `HealthCheck`), `POST /api/v1/lb/probe/{record_id}` (one-shot).
5. **Dashboard tab** — populate the existing LB tab from `/api/v1/lb/status`
   and the new WS event.
6. **WebSocket events** — emit `lb_state_change` on the existing dashboard
   WS whenever a record flips healthy/unhealthy or failsafe activates.
7. **Container manifest** — add `securityContext.capabilities.add: [NET_RAW]`
   to the DaemonSet rendered by mkube.

## 10. Open questions

All resolved — see "Approved decisions" at the top.
