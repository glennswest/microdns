#!/usr/bin/env python3
"""Sync all DNS records to mdns.gw.lo (192.168.1.52) from PowerDNS + MikroTik."""

import json
import sys
import urllib.request
import urllib.error

MDNS = "http://192.168.1.52:8080/api/v1"
PDNS = "http://192.168.1.51:8081/api/v1/servers/localhost"
PDNS_KEY = "quest.5124"


def mdns_get(path):
    req = urllib.request.Request(f"{MDNS}{path}")
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read())


def mdns_post(path, data):
    body = json.dumps(data).encode()
    req = urllib.request.Request(f"{MDNS}{path}", data=body, method="POST")
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        err = e.read().decode()
        if e.code == 409 or "already exists" in err.lower() or "duplicate" in err.lower():
            return None  # duplicate, skip
        print(f"  ERROR {e.code}: {err}")
        return None


def mdns_put(path, data):
    body = json.dumps(data).encode()
    req = urllib.request.Request(f"{MDNS}{path}", data=body, method="PUT")
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        print(f"  ERROR {e.code}: {e.read().decode()}")
        return None


def mdns_delete(path):
    req = urllib.request.Request(f"{MDNS}{path}", method="DELETE")
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status
    except urllib.error.HTTPError as e:
        print(f"  DELETE ERROR {e.code}: {e.read().decode()}")
        return e.code


def pdns_get(path):
    req = urllib.request.Request(f"{PDNS}{path}")
    req.add_header("X-API-Key", PDNS_KEY)
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError:
        return None


# ── Step 1: Get current mdns zones ──────────────────────────────────
print("=" * 60)
print("STEP 1: Inventory current mdns zones")
print("=" * 60)

zones = mdns_get("/zones")
zone_map = {}  # name -> list of {id, records_count}
for z in zones:
    name = z["name"]
    recs = mdns_get(f"/zones/{z['id']}/records?limit=500")
    count = len(recs) if recs else 0
    zone_map.setdefault(name, []).append({"id": z["id"], "name": name, "count": count})
    print(f"  {name}: {z['id'][:8]}... ({count} records)")

# ── Step 2: Delete duplicate zones (keep the one with most records) ──
print("\n" + "=" * 60)
print("STEP 2: Remove duplicate zones")
print("=" * 60)

for name, copies in zone_map.items():
    if len(copies) > 1:
        # Sort by count descending, keep the first
        copies.sort(key=lambda x: x["count"], reverse=True)
        keep = copies[0]
        print(f"  {name}: keeping {keep['id'][:8]}... ({keep['count']} recs)")
        for dup in copies[1:]:
            print(f"    deleting {dup['id'][:8]}... ({dup['count']} recs)")
            mdns_delete(f"/zones/{dup['id']}")

# Refresh zone list after cleanup
zones = mdns_get("/zones")
zone_by_name = {z["name"]: z["id"] for z in zones}
print(f"\n  Zones after cleanup: {len(zones)}")
for z in zones:
    print(f"    {z['name']} -> {z['id'][:8]}...")

# ── Step 3: Delete zones that should be forwarded, not local ──
print("\n" + "=" * 60)
print("STEP 3: Remove zones that will be forwarded (non-auth)")
print("=" * 60)

forward_zones_to_remove = [
    "g10.lo", "g11.lo", "gt.lo",
    "10.168.192.in-addr.arpa", "11.168.192.in-addr.arpa",
    "200.168.192.in-addr.arpa",
]
for zname in forward_zones_to_remove:
    if zname in zone_by_name:
        print(f"  Deleting local zone {zname} (will be forwarded)")
        mdns_delete(f"/zones/{zone_by_name[zname]}")
        del zone_by_name[zname]
    else:
        print(f"  {zname} not present (OK)")

# ── Step 4: Ensure required zones exist ──────────────────────────────
print("\n" + "=" * 60)
print("STEP 4: Ensure required zones exist")
print("=" * 60)

required_zones = [
    "gw.lo",
    "1.168.192.in-addr.arpa",
    "apps.gw.lo",
    "bm.lo",
    "pv.lo",
    "ipmi.lo",
    "g2.lo",
    "apps.g2.lo",
    "s1.lo",
    "ai.gw.lo",
    "33.19.10.in-addr.arpa",
    "28.19.10.in-addr.arpa",
    "0.22.172.in-addr.arpa",
    "2.168.192.in-addr.arpa",
]

for zname in required_zones:
    if zname not in zone_by_name:
        print(f"  Creating zone: {zname}")
        result = mdns_post("/zones", {"name": zname})
        if result:
            zone_by_name[zname] = result["id"]
    else:
        print(f"  {zname} exists")

# Refresh
zones = mdns_get("/zones")
zone_by_name = {z["name"]: z["id"] for z in zones}

# ── Step 5: Build target record sets from PowerDNS + MikroTik ──────
print("\n" + "=" * 60)
print("STEP 5: Build target records from PowerDNS + MikroTik")
print("=" * 60)

# Collect all records we want in gw.lo
# Format: {(name, type, data): ttl}
target_gw_records = {}


def add_record(name, rtype, data, ttl=300):
    """Add to target set. Strip trailing dots from data for A records."""
    if rtype in ("A", "AAAA"):
        data = data.rstrip(".")
    if rtype in ("CNAME", "NS", "PTR"):
        data = data.rstrip(".")
    key = (name, rtype, data)
    # Keep highest TTL
    if key not in target_gw_records or ttl > target_gw_records[key]:
        target_gw_records[key] = ttl


# -- From PowerDNS gw.lo zone --
print("  Fetching PowerDNS gw.lo records...")
pdns_gw = pdns_get("/zones/gw.lo.")
if pdns_gw:
    for rrset in pdns_gw.get("rrsets", []):
        rtype = rrset["type"]
        if rtype in ("SOA",):
            continue
        qname = rrset["name"].rstrip(".")
        # Convert FQDN to relative name
        if qname == "gw.lo":
            name = "@"
        elif qname.endswith(".gw.lo"):
            name = qname[: -len(".gw.lo")]
        else:
            continue

        ttl = rrset.get("ttl", 300)

        for rec in rrset.get("records", []):
            if rec.get("disabled", False):
                continue
            content = rec["content"].rstrip(".")

            if rtype == "A":
                add_record(name, "A", content, ttl)
            elif rtype == "NS":
                if name == "@":
                    add_record(name, "NS", content, ttl)
            elif rtype == "SRV":
                # SRV format: priority weight port target
                parts = content.split()
                if len(parts) == 4:
                    srv_data = {
                        "priority": int(parts[0]),
                        "weight": int(parts[1]),
                        "port": int(parts[2]),
                        "target": parts[3].rstrip("."),
                    }
                    key = (name, "SRV", json.dumps(srv_data, sort_keys=True))
                    target_gw_records[key] = ttl

    print(f"    Got {len(target_gw_records)} records from PowerDNS gw.lo")

# -- Fix dns.gw.lo → 192.168.1.52 --
add_record("dns", "A", "192.168.1.52", 300)
# Remove old dns.gw.lo pointing to .154
target_gw_records.pop(("dns", "A", "192.168.1.154"), None)

# -- Add mdns.gw.lo --
add_record("mdns", "A", "192.168.1.52", 300)

# -- Fix homekit.gw.lo -- old PowerDNS had it at .52, no longer valid --
# (was probably an old container, .52 is now mdns)
target_gw_records.pop(("homekit", "A", "192.168.1.52"), None)

# -- Remove buggy bootstrap.gw.lo.gw.lo --
target_gw_records.pop(("bootstrap.gw.lo", "A", "192.168.1.200"), None)

# -- From MikroTik DHCP static leases (;;; comment entries) --
print("  Adding MikroTik DHCP static hostname entries...")
mikrotik_hosts = {
    # From ;;; comment lines in DHCP lease table (static entries only)
    "monitor": "192.168.1.153",
    "bootstrap": "192.168.1.200",
    "network": "192.168.1.201",
    "control1": "192.168.1.202",
    "control2": "192.168.1.203",
    "worker0": "192.168.1.204",
    "worker1": "192.168.1.205",
    "worker2": "192.168.1.206",
    "dev": "192.168.1.151",
    "workmac": "192.168.1.18",
    "cap01": "192.168.1.14",
    "generator": "192.168.1.32",
    "nvr": "192.168.1.40",
    "epson": "192.168.1.33",
    "frame1": "192.168.1.30",
    "bay2": "192.168.1.76",
    "washerdryer": "192.168.1.78",
    "espx": "192.168.1.79",
    "graylog": "192.168.1.87",
    "minio": "192.168.1.55",
    "pbs": "192.168.1.165",
    "storex": "192.168.1.161",
    "traefik": "192.168.1.168",
    "standb": "192.168.1.72",
    "logs": "192.168.1.92",
    "boot": "192.168.1.5",
    "rhel9full": "192.168.1.175",
    "sv08": "192.168.1.106",
    "hub": "192.168.1.115",
    "vweb": "192.168.1.116",
    "bay1": "192.168.1.118",
    "ecoflow": "192.168.1.17",
    "registry": "192.168.1.80",
    "blink42": "192.168.1.23",
    "blink49": "192.168.1.31",
    "ting": "192.168.1.9",
    "cap02": "192.168.1.4",
    "rose1": "192.168.1.88",
    "cap03": "192.168.1.252",
    "naman": "192.168.1.135",
    "cp0.thi": "192.168.1.210",
    "cp1.thi": "192.168.1.211",
    "cp2.thi": "192.168.1.212",
}

for name, ip in mikrotik_hosts.items():
    add_record(name, "A", ip, 300)

print(f"    Total target gw.lo records: {len(target_gw_records)}")

# ── Step 6: Sync gw.lo records ──────────────────────────────────────
print("\n" + "=" * 60)
print("STEP 6: Sync gw.lo records")
print("=" * 60)

gw_zone_id = zone_by_name["gw.lo"]
existing_recs = mdns_get(f"/zones/{gw_zone_id}/records?limit=500")

# Build set of existing (name, type, data)
existing_set = set()
existing_by_key = {}
for r in existing_recs:
    rdata = r.get("data", {})
    rtype = rdata.get("type", "")
    if rtype == "A":
        data_val = rdata.get("data", "")
    elif rtype == "NS":
        data_val = rdata.get("data", "")
    elif rtype == "SRV":
        data_val = json.dumps(rdata.get("data", {}), sort_keys=True)
    elif rtype == "PTR":
        data_val = rdata.get("data", "")
    else:
        data_val = str(rdata.get("data", ""))
    key = (r["name"], rtype, data_val)
    existing_set.add(key)
    existing_by_key[key] = r["id"]

# Add missing records
added = 0
skipped = 0
for (name, rtype, data_val), ttl in target_gw_records.items():
    key = (name, rtype, data_val)
    if key in existing_set:
        skipped += 1
        continue

    # Build record data
    if rtype == "A":
        rec_data = {"type": "A", "data": data_val}
    elif rtype == "NS":
        rec_data = {"type": "NS", "data": data_val}
    elif rtype == "SRV":
        rec_data = {"type": "SRV", "data": json.loads(data_val)}
    else:
        continue

    result = mdns_post(f"/zones/{gw_zone_id}/records", {
        "name": name,
        "ttl": ttl,
        "data": rec_data,
        "enabled": True,
    })
    if result:
        print(f"  + {name}.gw.lo {rtype} {data_val} (TTL {ttl})")
        added += 1
    else:
        skipped += 1

print(f"\n  Added: {added}, Skipped (existing/dup): {skipped}")

# -- Fix dns.gw.lo if it exists with old IP --
old_dns_key = ("dns", "A", "192.168.1.154")
if old_dns_key in existing_by_key:
    rid = existing_by_key[old_dns_key]
    print(f"  Updating dns.gw.lo from .154 to .52 (record {rid[:8]}...)")
    mdns_put(f"/zones/{gw_zone_id}/records/{rid}", {
        "data": {"type": "A", "data": "192.168.1.52"},
        "ttl": 300,
    })

# -- Delete bogus records --
bogus_keys = [
    ("bootstrap.gw.lo", "A", "192.168.1.200"),  # double-domain
]
for bk in bogus_keys:
    if bk in existing_by_key:
        print(f"  Deleting bogus: {bk[0]}.gw.lo {bk[1]} {bk[2]}")
        mdns_delete(f"/zones/{gw_zone_id}/records/{existing_by_key[bk]}")

# ── Step 7: Rebuild reverse DNS ─────────────────────────────────────
print("\n" + "=" * 60)
print("STEP 7: Rebuild reverse DNS (1.168.192.in-addr.arpa)")
print("=" * 60)

rev_zone_id = zone_by_name.get("1.168.192.in-addr.arpa")
if not rev_zone_id:
    print("  ERROR: reverse zone not found!")
    sys.exit(1)

# First, delete ALL existing PTR records in reverse zone (clean rebuild)
rev_recs = mdns_get(f"/zones/{rev_zone_id}/records?limit=500")
ptr_count = 0
for r in rev_recs:
    if r.get("data", {}).get("type") == "PTR":
        mdns_delete(f"/zones/{rev_zone_id}/records/{r['id']}")
        ptr_count += 1
    elif r.get("data", {}).get("type") == "NS":
        mdns_delete(f"/zones/{rev_zone_id}/records/{r['id']}")

print(f"  Cleared {ptr_count} old PTR records")

# Build PTR records from all gw.lo A records
# Collect unique (octet -> hostname) mappings
# For IPs in 192.168.1.x, the PTR name is the last octet
ptr_map = {}  # last_octet -> list of hostnames
for (name, rtype, data_val), ttl in target_gw_records.items():
    if rtype != "A":
        continue
    if not data_val.startswith("192.168.1."):
        continue
    octet = data_val.split(".")[3]
    hostname = f"{name}.gw.lo"
    if hostname.startswith("@."):
        continue
    # Skip wildcard records
    if "*" in name:
        continue
    # Clean up any double-domain
    if ".gw.lo.gw.lo" in hostname:
        continue
    ptr_map.setdefault(octet, set()).add(hostname)

# Create PTR records (one per hostname per octet)
ptr_added = 0
for octet in sorted(ptr_map.keys(), key=lambda x: int(x)):
    for hostname in sorted(ptr_map[octet]):
        result = mdns_post(f"/zones/{rev_zone_id}/records", {
            "name": octet,
            "ttl": 300,
            "data": {"type": "PTR", "data": hostname},
            "enabled": True,
        })
        if result:
            ptr_added += 1

print(f"  Created {ptr_added} PTR records")

# ── Step 8: Sync other zones from PowerDNS ──────────────────────────
print("\n" + "=" * 60)
print("STEP 8: Sync utility zones from PowerDNS")
print("=" * 60)

other_zones = {
    "bm.lo": "bm.lo.",
    "pv.lo": "pv.lo.",
    "ipmi.lo": "ipmi.lo.",
    "g2.lo": "g2.lo.",
    "apps.g2.lo": "apps.g2.lo.",
    "apps.gw.lo": "apps.gw.lo.",
    "33.19.10.in-addr.arpa": "33.19.10.in-addr.arpa.",
    "28.19.10.in-addr.arpa": "28.19.10.in-addr.arpa.",
    "0.22.172.in-addr.arpa": "0.22.172.in-addr.arpa.",
    "2.168.192.in-addr.arpa": "2.168.192.in-addr.arpa.",
}

for mdns_name, pdns_name in other_zones.items():
    zid = zone_by_name.get(mdns_name)
    if not zid:
        print(f"  {mdns_name}: zone not found, skipping")
        continue

    pdns_zone = pdns_get(f"/zones/{pdns_name}")
    if not pdns_zone:
        print(f"  {mdns_name}: not in PowerDNS, skipping")
        continue

    # Get existing records
    existing = mdns_get(f"/zones/{zid}/records?limit=500")
    existing_keys = set()
    for r in existing:
        rd = r.get("data", {})
        existing_keys.add((r["name"], rd.get("type", ""), str(rd.get("data", ""))))

    zone_added = 0
    for rrset in pdns_zone.get("rrsets", []):
        rtype = rrset["type"]
        if rtype in ("SOA",):
            continue
        qname = rrset["name"].rstrip(".")
        ttl = rrset.get("ttl", 300)

        # Convert to relative name
        suffix = f".{mdns_name}"
        if qname == mdns_name:
            name = "@"
        elif qname.endswith(suffix):
            name = qname[: -len(suffix)]
        else:
            continue

        for rec in rrset.get("records", []):
            if rec.get("disabled", False):
                continue
            content = rec["content"].rstrip(".")

            if rtype == "A":
                rec_data = {"type": "A", "data": content}
                check_key = (name, "A", content)
            elif rtype == "NS":
                rec_data = {"type": "NS", "data": content}
                check_key = (name, "NS", content)
            elif rtype == "PTR":
                rec_data = {"type": "PTR", "data": content}
                check_key = (name, "PTR", content)
            elif rtype == "SRV":
                parts = content.split()
                if len(parts) != 4:
                    continue
                srv = {
                    "priority": int(parts[0]),
                    "weight": int(parts[1]),
                    "port": int(parts[2]),
                    "target": parts[3].rstrip("."),
                }
                rec_data = {"type": "SRV", "data": srv}
                check_key = (name, "SRV", json.dumps(srv, sort_keys=True))
            else:
                continue

            if check_key in existing_keys:
                continue

            result = mdns_post(f"/zones/{zid}/records", {
                "name": name,
                "ttl": ttl,
                "data": rec_data,
                "enabled": True,
            })
            if result:
                zone_added += 1

    print(f"  {mdns_name}: added {zone_added} records")

# ── Step 9: Clean up s1.lo duplicates ────────────────────────────────
print("\n" + "=" * 60)
print("STEP 9: Clean up s1.lo duplicate records")
print("=" * 60)

s1_zid = zone_by_name.get("s1.lo")
if s1_zid:
    s1_recs = mdns_get(f"/zones/{s1_zid}/records?limit=500")
    # Group by (name, type, data)
    s1_groups = {}
    for r in s1_recs:
        rd = r.get("data", {})
        key = (r["name"], rd.get("type", ""), str(rd.get("data", "")))
        s1_groups.setdefault(key, []).append(r["id"])

    deleted = 0
    for key, ids in s1_groups.items():
        if len(ids) > 1:
            # Keep first, delete rest
            for rid in ids[1:]:
                mdns_delete(f"/zones/{s1_zid}/records/{rid}")
                deleted += 1

    print(f"  Deleted {deleted} duplicate s1.lo records")

# ── Step 10: Clean up duplicate records in gw.lo ─────────────────────
print("\n" + "=" * 60)
print("STEP 10: Clean up duplicate records in gw.lo")
print("=" * 60)

gw_recs = mdns_get(f"/zones/{gw_zone_id}/records?limit=500")
gw_groups = {}
for r in gw_recs:
    rd = r.get("data", {})
    key = (r["name"], rd.get("type", ""), str(rd.get("data", "")))
    gw_groups.setdefault(key, []).append(r["id"])

dup_deleted = 0
for key, ids in gw_groups.items():
    if len(ids) > 1:
        for rid in ids[1:]:
            mdns_delete(f"/zones/{gw_zone_id}/records/{rid}")
            dup_deleted += 1
        if dup_deleted <= 20:  # only print first 20
            print(f"  Deduped: {key[0]} {key[1]} {key[2]}")

print(f"  Deleted {dup_deleted} duplicate gw.lo records")

# ── Summary ──────────────────────────────────────────────────────────
print("\n" + "=" * 60)
print("SYNC COMPLETE")
print("=" * 60)

# Final zone count
final_zones = mdns_get("/zones")
print(f"  Total zones: {len(final_zones)}")
for z in sorted(final_zones, key=lambda x: x["name"]):
    recs = mdns_get(f"/zones/{z['id']}/records?limit=500")
    print(f"    {z['name']}: {len(recs)} records")
