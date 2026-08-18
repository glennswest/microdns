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

```toml
[mdns]
enabled = true
zone = "mdns.g9.lo"     # required — where discovered names land
```

A device announcing `teslatracker-52c4.local` on that segment becomes:

```
teslatracker-52c4.mdns.g9.lo.  120  IN  A  192.168.9.134
```

Resolvable from any subnet that can reach this instance. For clients on other
networks, add the usual forward zone entry so queries reach the right instance:

```toml
[dns.recursor.forward_zones]
"mdns.g9.lo" = ["192.168.9.252:53"]
```

### Why a dedicated subzone

`zone` can be any zone, but a subzone (`mdns.<network>.lo`) is the recommended
shape: it keeps auto-discovered names visibly separate from curated ones, and
it is obvious at a glance where a name came from. Pointing `zone` at the
network's main zone works too — discovered records are labelled `source: mdns`
either way, and curated records in that zone are never touched.

Note that publishing into a zone literally named `local` is *not* useful in
practice: macOS and systemd-resolved route `.local` to multicast only and never
ask a unicast resolver, so those queries would never arrive.

## Configuration

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `false` | Off unless asked for — an upgrade must not start publishing whatever is shouting on the LAN |
| `zone` | *(required)* | Zone discovered names are published into |
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

Filtering a noisy network down to what matters:

```toml
[mdns]
enabled = true
zone = "mdns.g9.lo"
deny = ["chromecast-*", "*._sleep-proxy._udp"]
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
