# mDNS Ingest

Bridges mDNS (`.local`) announcements into authoritative unicast DNS, so names
that only exist on one segment become resolvable from the whole network.

## The problem it solves

mDNS queries go to `224.0.0.251` with an **IP TTL of 1**. Routers drop them by
design, so a device advertising `teslatracker-52c4.local` is invisible by name
to every client outside its own subnet — even though its address is perfectly
routable and reachable.

The usual workarounds both have costs:

| Approach | Cost |
|---|---|
| mDNS reflector on the router | per-vendor config on every VLAN pair, floods segments with traffic clients discard |
| Manual A record + DHCP reservation per device | exactly the hand-maintenance MicroDNS exists to remove, and goes stale silently |

A MicroDNS instance already sits on the segment it serves — which is precisely
where the announcements are audible. It listens, holds what it hears for as
long as the announced TTL says to, and publishes the result as ordinary
authoritative records. Cross-subnet clients then resolve those names over
normal unicast DNS.

## Enabling it

Configuration lives in the database and is managed through the API. That is
deliberate: on mkube-managed instances `microdns.toml` is generated from a
Network CRD, so a `[mdns]` block added there is discarded the next time the
config is regenerated. The stored config survives it.

```bash
curl -s -X PUT http://192.168.9.252:8080/api/v1/mdns/config \
  -H 'Content-Type: application/json' \
  -d '{"enabled": true}'
```

Run that on every instance and you get **one flat domain**, `mdns.lo`, that
answers for devices on any network. Each instance publishes what it hears on
its own segment and mirrors what its siblings hear, so the same name resolves
the same way from anywhere — no network in the name, and no need to know which
subnet a device sits on.

The running source picks the change up within ten seconds — starting, stopping
or re-homing its zone with no restart. `DELETE /api/v1/mdns/config` turns it off
and withdraws every name it published.

A `[mdns]` block in the config file is still read, but only to **seed** the
stored value the first time, the way DHCP pools and forwarders are seeded:

```toml
[mdns]
enabled = true
zone = "mdns.g9.lo"     # required — where discovered names land
```

Once the value is stored, the file no longer has a say — edit it through the API.

A device announcing `teslatracker-52c4.local` on g9 becomes:

```
teslatracker-52c4.mdns.lo.  120  IN  A  192.168.9.134
```

resolvable from every instance, not just g9's.

### How one domain covers every network

mDNS is only audible on the segment it was announced on, so each instance hears
a different slice. Rather than making that slicing visible in the name, one
instance **holds** the zone and every other instance **registers into it**:

```
g8  hears glenns-mac-mini ──┐
g9  hears teslatracker-52c4 ─┼─→  dns.mdns.lo  holds mdns.lo
gw  hears cap01            ──┘         ↑
                        every instance forwards mdns.lo here
```

A reporting instance holds no copy of the zone at all. It writes what it hears
straight into the holder over the REST API — records land with `source: mdns` —
and points its own clients at the holder for that zone, so a lookup arriving
anywhere is answered by the one box that has the names.

Each instance remembers the record ids it created, so it withdraws its own names
when a device goes away and never touches another segment's. That memory is
persisted locally, so a restart does not orphan anything.

The holder is set per instance:

```bash
# the instance that holds the zone
-d '{"enabled": true, "zone": "mdns.lo"}'

# every other instance
-d '{"enabled": true, "zone": "mdns.lo", "holder": "192.168.12.252"}'
```

The holder's API is expected on 8080 and its DNS on 53, as every instance
serves.

### Choosing a different shape

`zone` can be anything, and an instance with no `holder` set simply keeps what
it hears in its own copy. That gives per-network zones (`mdns.g9.lo` on g9) if
you want the network visible in the name — but then callers have to know which
segment a device lives on, which is the bookkeeping this exists to remove.

Note that publishing into a zone literally named `local` is *not* useful in
practice: macOS and systemd-resolved route `.local` to multicast only and never
ask a unicast resolver, so those queries would never arrive.

### Name collisions

Two devices on different networks announcing the same name both get published,
producing one RRset with both addresses. Nothing arbitrates between them — the
flat domain is a flat namespace, and `deny` is the tool for excluding a noisy
duplicate.

## Configuration

Every key below is a field of the `PUT /api/v1/mdns/config` body (and of the
bootstrap `[mdns]` block). `PUT` replaces the whole object, so send the full
config, not a fragment.

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `false` | Off unless asked for — an upgrade must not start publishing whatever is shouting on the LAN |
| `zone` | `"mdns.lo"` | Zone discovered names are published into — one flat domain shared by every instance |
| `ttl_min` | `60` | Floor for the announced TTL. Responses to a legacy query carry 10 s (RFC 6762 §6.7), which would churn the zone |
| `ttl_max` | `1200` | Ceiling, so a device advertising a huge TTL cannot pin a stale record |
| `services` | `true` | Also mirror DNS-SD records (PTR/SRV/TXT), not just addresses |
| `allow` | `[]` | Glob patterns (`*`) of names to publish. Empty allows all |
| `deny` | `[]` | Glob patterns never to publish. Checked after `allow`, so deny wins |
| `query_interval_secs` | `300` | How often to send DNS-SD enumeration queries. `0` = passive only |
| `ipv6` | `false` | Also join `ff02::fb`. Responders answering over IPv6 generally answer over IPv4 too |
| `bind` | `"0.0.0.0"` | Listener address |
| `interfaces` | `[]` | Interface addresses to join the group on. Empty lets the kernel choose |
| `debounce_secs` | `5` | Quiet window before a burst of announcements is written to the zone |
| `holder` | `""` | Address of the instance holding the zone. Empty means this instance holds it |

Filtering a noisy network down to what matters:

```bash
curl -s -X PUT http://192.168.9.252:8080/api/v1/mdns/config \
  -H 'Content-Type: application/json' \
  -d '{"enabled": true, "zone": "mdns.g9.lo",
       "deny": ["chromecast-*", "*._sleep-proxy._udp"]}'
```

## How discovery works

1. **Passive listening.** Every mDNS *response* on the segment is parsed.
   Queries are ignored: a query's authority section holds records a host is
   merely *proposing* to claim (RFC 6762 §8.1), and its answer section holds
   known-answer suppression — neither asserts that a record exists.
2. **Active browsing.** Every `query_interval_secs`, the source sends the
   DNS-SD meta-query `_services._dns-sd._udp.local` and then browses each
   service type it has learned. This is what populates the table without
   waiting for a device to re-announce.
3. **Cache maintenance.** A record that has used 80% of its lifetime is
   re-queried rather than dropped (RFC 6762 §5.2). Devices generally announce
   only at startup, so without this a name learned once would vanish two
   minutes later.
4. **Withdrawal.** A goodbye packet (TTL 0) removes the record immediately; so
   does the lifetime running out with no answer. The zone follows.

Announcements are read from both the answer and additional sections — a
responder answering a browse puts the SRV/TXT/A it knows in additionals, and
ignoring them would mean a second round trip for data already in hand.

### What is deliberately dropped

- Names outside `.local`.
- Link-local (`169.254.0.0/16`, `fe80::/10`), loopback and unspecified
  addresses. They only mean something on the segment they were announced on;
  publishing them would hand cross-subnet clients an address that cannot work.
- Record types MicroDNS does not serve.

### Name rewriting

Names inside rdata are rewritten into the publish zone, so a DNS-SD browse that
starts in the zone stays in the zone rather than walking back out to `.local`:

```
_ssh._tcp.mdns.g9.lo.            PTR  gwest-mac._ssh._tcp.mdns.g9.lo.
gwest-mac._ssh._tcp.mdns.g9.lo.  SRV  0 0 22 gwest-mac.mdns.g9.lo.
gwest-mac.mdns.g9.lo.            A    192.168.8.103
```

DNS-SD packs one `key=value` per character-string in a TXT record; MicroDNS
stores a single string per record, so those strings are joined with spaces —
the way `dig` renders them. Keys stay readable and splittable on whitespace.

## Discovered vs curated records

Every record carries a `source` (`manual`, `dhcp`, `mdns`, `k8s`), visible in
the REST API:

```json
{ "name": "teslatracker-52c4", "type": "A", "source": "mdns", ... }
```

Two rules follow from it, and both are enforced on every reconcile:

- **A curated record always wins.** If a `manual` record already owns a name
  and type, the discovered one is not published, and the clash is logged once.
- **Only discovered records are pruned.** The source deletes nothing it did not
  create, so pointing `zone` at a zone with hand-made records is safe.

After a restart the cache is empty simply because nothing has announced yet, so
withdrawal is held back for the first 45 seconds. Publishing starts with the
first packet; without the grace window a restart would drop every discovered
name and re-add it seconds later.

## Operating it

```bash
# The stored config (404 until the source has been configured)
curl -s http://192.168.9.252:8080/api/v1/mdns/config

# Counters, cache size, service types seen on the segment
curl -s http://192.168.9.252:8080/api/v1/mdns/status

# The live discovery table — including names filtered out by config,
# and where each one is published
curl -s http://192.168.9.252:8080/api/v1/mdns/discovered
```

`discovered` shows what was *heard*; the zone/record endpoints show what was
*published*. When a device announces but does not resolve, the difference
between those two is the answer — `published_as: null` means a filter dropped
it.

## As deployed here

| | |
|---|---|
| Network | `mdns` — `192.168.12.0/24`, `bridge-mdns`, no DHCP |
| Holder | `dns.mdns.lo` (192.168.12.252), holds `mdns.lo` |
| Reporting | gw, g8, g9, g10, g11, g16, g100 |
| Not participating | gt |

A device announcing on any of those segments is reachable as
`<name>.mdns.lo` from all of them, at whatever address it has on its own
network.

## Requirements and caveats

- **Port 5353.** The listener binds it with `SO_REUSEADDR`/`SO_REUSEPORT`, so it
  coexists with another mDNS stack on the host. A recursor configured on 5353
  would take a share of the queries; MicroDNS logs a warning if both are set
  that way. Deployed instances run the recursor on `:53`.
- **The container must be on the LAN.** Multicast has to reach the instance —
  macvlan or bridged networking, not NAT.
- **Reverse (PTR) records are not created** for discovered addresses. Those
  ranges are usually owned by DHCP registration already, and two sources
  fighting over one PTR is worse than not having it.
