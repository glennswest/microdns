# Zone Transfer (AXFR) and NOTIFY

How a second MicroDNS instance keeps a live copy of another's zone, and how it
finds out that the copy is stale within seconds rather than within an hour.

## The two halves

| Direction | Config | What it does |
|---|---|---|
| Primary → secondary | `[dns.auth] notify` | Announces "this zone changed" to each secondary the moment it happens |
| Secondary → primary | `[[dns.auth.secondary]]` | Mirrors the zone over AXFR, driven by those announcements and by a fallback timer |

Both are needed. A NOTIFY nobody acts on is noise; a secondary with no NOTIFY is
only as fresh as its refresh interval — 3600 s on these zones, which is exactly
the window in which a fallback server would be serving visibly wrong data at the
moment it is being relied on.

## Primary

```toml
[dns.auth]
enabled = true
listen = "0.0.0.0:53"
zones = ["gw.lo"]

# Who may pull a full copy. Default is RFC1918 + loopback.
allow_transfer = ["192.168.0.0/16", "127.0.0.0/8"]

# Who to tell when a zone changes. `ip` or `ip:port`.
notify = ["192.168.1.51", "192.168.8.253:53"]
```

Every writer in MicroDNS ends a change by bumping the zone's SOA serial — the
REST API, DHCP auto-registration, the mDNS and Kubernetes sources, reverse-PTR
sync. That single call is where the announcement hooks in, so **all** of them
are covered without any of them knowing NOTIFY exists.

Changes are collected for two seconds and collapsed to one NOTIFY per zone: a
single API call can bump a serial two or three times (forward record, reverse
PTR), and a secondary should not transfer a zone three times for one edit.

A NOTIFY is a hint, not a delivery guarantee (RFC 1996 §4). A lost one costs
latency, not correctness — the secondary's refresh timer still catches it — so
failures are logged and never block the write that triggered them.

## Secondary

```toml
[dns.auth]
enabled = true          # required: this is what receives the NOTIFY
listen = "0.0.0.0:53"

[[dns.auth.secondary]]
zone = "gw.lo"
primary = "192.168.1.252"
refresh_secs = 900      # fallback poll, for when a NOTIFY goes missing
```

On startup every mirrored zone is checked at once, so an instance that was down
comes back current immediately. After that a zone is checked when its primary
announces a change, and otherwise every `refresh_secs`.

A check is cheap and does not imply a transfer:

1. Ask the primary for the zone's SOA over UDP.
2. Compare its serial with the local copy, using RFC 1982 arithmetic — serials
   wrap, so "bigger number" is not the same as "newer".
3. Transfer only if they differ. A primary whose serial went *backwards* (a
   restore, or a zone rebuilt from scratch) is mirrored anyway and logged:
   matching the primary is the job, and refusing would strand the copy.

If the primary is unreachable, that is a warning and not an error — the local
copy keeps being served, which is the entire point of having one.

The transferred zone is swapped in atomically: the zone record is upserted and
its contents replaced in one pass, so there is no window where this server
answers "no such zone" for a zone it holds a perfectly good copy of.

## Security

**AXFR hands over a complete map of internal hosts**, so the primary refuses one
from any address outside `allow_transfer` before it reads a single record. An
empty list denies everything; an unparseable entry is dropped with a warning
(a typo in an ACL must not take DNS down, and dropping denies rather than
permits).

**A NOTIFY is an instruction to go and transfer a zone**, so a secondary
believes it only from that zone's configured `primary` address. Anything else —
wrong sender, or a zone this instance does not mirror — gets REFUSED. Without
that check, any host on the network could aim this instance's transfers wherever
it liked.

Transfers are also chunked into messages of 100 records (RFC 5936 §2.2). Packing
a whole zone into one message caps it at the 16-bit TCP length prefix, where the
length would silently wrap rather than error — a corrupt transfer that is very
hard to diagnose.

## Operating it

```bash
# Pull a zone by hand (one-shot; same path the secondary agent uses)
curl -s -X POST http://192.168.1.51:8080/api/v1/zones/transfer \
  -H 'Content-Type: application/json' \
  -d '{"zone": "gw.lo", "primary": "192.168.1.252:53"}'

# What the secondary currently holds
dig @192.168.1.51 gw.lo SOA +short

# What the primary says it should be
dig @192.168.1.252 gw.lo SOA +short
```

The serials matching is the whole health check: if they differ for longer than
`refresh_secs`, look for a refused transfer in the secondary's logs
(`GET /api/v1/logs?level=warn`) — an `allow_transfer` miss on the primary is by
far the most common cause.

### Log lines worth knowing

| Line | Meaning |
|---|---|
| `NOTIFY accepted for <zone> from <ip>` | The announcement was believed; a check follows |
| `NOTIFY for <zone> refused: <ip> is not the configured primary` | Sender does not match `primary` for that zone |
| `AXFR for <zone> refused: <ip> is not in allow_transfer` | On the primary — widen the ACL |
| `secondary: <zone> — could not read SOA from <ip>` | Primary unreachable; the existing copy is still served |

## Records and provenance

Records arriving by transfer are stored with `source: manual`. A mirror is a
copy of someone else's decisions, and the automatic sources on the secondary
(DHCP, mDNS, Kubernetes) must not treat mirrored records as theirs to prune.

Note that a transfer replaces the zone's entire contents. Do not point a
secondary at a zone that this instance also writes to locally — the next
transfer will discard whatever it wrote.
