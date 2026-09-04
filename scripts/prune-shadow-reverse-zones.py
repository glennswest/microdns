#!/usr/bin/env python3
"""Delete shadow reverse zones — local copies of *another* network's
in-addr.arpa zone — from each microdns instance.

Before v0.9.5, one A record pointing across networks (fastregistry.g8.lo ->
192.168.10.50, an IPMI host named on g16, an mDNS device on gw seen by the
mdns holder) auto-created a one-record reverse zone for that network on the
instance that held the A record. hickory answers from a local zone before it
consults a forwarder, so that zone turned every other address in the /24 into
an authoritative NXDOMAIN there. v0.9.5 stops creating them; this removes the
ones already on disk. Safe to re-run: an absent zone is reported and skipped.

An instance owns the /24 reverse zones covered by its DHCP pool subnets plus
the ones listed in OWNED below (instances with DHCP off, and 127.0.0.0/8).
Everything else ending in .168.192.in-addr.arpa is deleted, and the records
it held with it — they were only ever the stray PTRs for the foreign address.

    ./scripts/prune-shadow-reverse-zones.py            # dry run, prints the plan
    ./scripts/prune-shadow-reverse-zones.py --apply    # delete
"""

import ipaddress
import json
import sys
import urllib.request

INSTANCES = [
    "192.168.1.252",    # gw
    "192.168.8.252",    # g8
    "192.168.9.252",    # g9
    "192.168.10.252",   # g10
    "192.168.11.252",   # g11
    "192.168.31.252",   # g16 (flat /20, services at the top)
    "192.168.100.252",  # g100
    "192.168.200.199",  # gt
    "192.168.12.252",   # mdns holder
]

# Reverse zones an instance owns regardless of DHCP pools.
OWNED = {
    "192.168.200.199": {"200.168.192.in-addr.arpa"},
    "192.168.100.252": {"100.168.192.in-addr.arpa"},
    "192.168.12.252": {"12.168.192.in-addr.arpa"},
}


def get(url):
    with urllib.request.urlopen(url, timeout=5) as r:
        return json.load(r)


def owned_reverse_zones(ip):
    owned = set(OWNED.get(ip, ()))
    for pool in get(f"http://{ip}:8080/api/v1/dhcp/pools"):
        net = ipaddress.ip_network(pool["subnet"], strict=False)
        if net.version != 4:
            continue
        for sub in net.subnets(new_prefix=24) if net.prefixlen <= 24 else [net]:
            o = str(sub.network_address).split(".")
            owned.add(f"{o[2]}.{o[1]}.{o[0]}.in-addr.arpa")
    return owned


def main():
    apply = "--apply" in sys.argv[1:]
    total = 0
    for ip in INSTANCES:
        try:
            owned = owned_reverse_zones(ip)
            zones = get(f"http://{ip}:8080/api/v1/zones")
        except Exception as e:  # noqa: BLE001
            print(f"{ip:16s} unreachable: {e}")
            continue
        shadow = [
            z for z in zones
            if z["name"].endswith(".168.192.in-addr.arpa") and z["name"] not in owned
        ]
        if not shadow:
            print(f"{ip:16s} clean (owns {len(owned)})")
            continue
        for z in shadow:
            total += 1
            label = f"{z['name']} ({z['record_count']} rec)"
            if apply:
                req = urllib.request.Request(
                    f"http://{ip}:8080/api/v1/zones/{z['id']}", method="DELETE")
                with urllib.request.urlopen(req, timeout=5) as r:
                    print(f"{ip:16s} deleted {label}: HTTP {r.status}")
            else:
                print(f"{ip:16s} would delete {label}")
    if not apply and total:
        print(f"\n{total} shadow zone(s). Re-run with --apply to delete them.")


if __name__ == "__main__":
    main()
