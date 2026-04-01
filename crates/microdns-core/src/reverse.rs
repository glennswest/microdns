//! Automatic reverse DNS zone management.
//!
//! Computes reverse zone names and PTR record names from IP addresses,
//! auto-creates reverse zones when needed, and synchronizes PTR records
//! as A/AAAA records are created, updated, or deleted.

use crate::db::Db;
use crate::error::Result;
use crate::types::{Record, RecordData, RecordType, SoaData, Zone};
use chrono::Utc;
use std::net::{Ipv4Addr, Ipv6Addr};
use tracing::debug;
use uuid::Uuid;

/// Compute the /24 reverse zone name for an IPv4 address.
/// e.g., 192.168.10.5 → "10.168.192.in-addr.arpa"
pub fn reverse_zone_v4(ip: Ipv4Addr) -> String {
    let o = ip.octets();
    format!("{}.{}.{}.in-addr.arpa", o[2], o[1], o[0])
}

/// Compute the PTR record name (last octet) for an IPv4 address.
/// e.g., 192.168.10.5 → "5"
pub fn ptr_name_v4(ip: Ipv4Addr) -> String {
    ip.octets()[3].to_string()
}

/// Compute the /64 reverse zone name for an IPv6 address.
/// e.g., 2001:db8::1 → "0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa"
pub fn reverse_zone_v6(ip: Ipv6Addr) -> String {
    let nibbles = ipv6_nibbles(ip);
    // /64 zone = first 16 nibbles (MSB first), reversed for DNS
    let zone_part: Vec<&str> = nibbles[..16].iter().rev().map(|s| s.as_str()).collect();
    format!("{}.ip6.arpa", zone_part.join("."))
}

/// Compute the PTR record name for an IPv6 address within its /64 zone.
/// e.g., 2001:db8::1 → "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0"
pub fn ptr_name_v6(ip: Ipv6Addr) -> String {
    let nibbles = ipv6_nibbles(ip);
    // Host part = last 16 nibbles (MSB first), reversed for DNS
    let host_part: Vec<&str> = nibbles[16..].iter().rev().map(|s| s.as_str()).collect();
    host_part.join(".")
}

/// Expand an IPv6 address to 32 hex nibbles (MSB first).
fn ipv6_nibbles(ip: Ipv6Addr) -> Vec<String> {
    ip.segments()
        .iter()
        .flat_map(|s| {
            vec![
                format!("{:x}", (s >> 12) & 0xf),
                format!("{:x}", (s >> 8) & 0xf),
                format!("{:x}", (s >> 4) & 0xf),
                format!("{:x}", s & 0xf),
            ]
        })
        .collect()
}

/// Check if a zone name is a reverse zone (in-addr.arpa or ip6.arpa).
pub fn is_reverse_zone(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".in-addr.arpa")
        || lower.ends_with(".ip6.arpa")
        || lower == "in-addr.arpa"
        || lower == "ip6.arpa"
}

/// Construct the FQDN for a PTR target from a record name and zone name.
/// "@" (zone apex) → "zone_name." ; "www" → "www.zone_name."
fn ptr_target(record_name: &str, zone_name: &str) -> String {
    let zone = zone_name.trim_end_matches('.');
    if record_name == "@" {
        format!("{zone}.")
    } else {
        format!("{record_name}.{zone}.")
    }
}

/// Ensure a reverse zone exists, creating it if necessary.
pub fn ensure_reverse_zone(db: &Db, zone_name: &str) -> Result<Zone> {
    if let Some(zone) = db.get_zone_by_name(zone_name)? {
        return Ok(zone);
    }

    let zone = Zone {
        id: Uuid::new_v4(),
        name: zone_name.to_string(),
        soa: SoaData {
            mname: format!("ns1.{zone_name}"),
            rname: format!("admin.{zone_name}"),
            serial: Utc::now()
                .format("%Y%m%d00")
                .to_string()
                .parse()
                .unwrap_or(1),
            refresh: 3600,
            retry: 900,
            expire: 604800,
            minimum: 300,
        },
        default_ttl: 300,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    db.create_zone(zone_name, &zone)?;
    debug!("auto-created reverse zone: {zone_name}");
    Ok(zone)
}

/// Create or update the PTR record for an A record.
/// Auto-creates the reverse zone if it doesn't exist.
pub fn sync_ptr_for_a(
    db: &Db,
    record_name: &str,
    forward_zone_name: &str,
    ip: Ipv4Addr,
    ttl: u32,
) -> Result<()> {
    let rev_zone_name = reverse_zone_v4(ip);
    let rev_zone = ensure_reverse_zone(db, &rev_zone_name)?;
    let ptr_name = ptr_name_v4(ip);
    let target = ptr_target(record_name, forward_zone_name);
    let ptr_data = RecordData::PTR(target.clone());

    // If identical PTR already exists, skip
    let existing = db.query_records(&rev_zone.id, &ptr_name, RecordType::PTR)?;
    if existing.iter().any(|r| r.data == ptr_data) {
        return Ok(());
    }

    // Delete existing PTR for this IP (one IP = one PTR)
    for rec in &existing {
        db.delete_record(&rec.id)?;
    }

    let ptr_record = Record {
        id: Uuid::new_v4(),
        zone_id: rev_zone.id,
        name: ptr_name.clone(),
        ttl,
        data: ptr_data,
        enabled: true,
        health_check: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    db.create_record(&ptr_record)?;
    db.increment_soa_serial(&rev_zone.id)?;
    debug!("synced PTR: {ptr_name}.{rev_zone_name} -> {target}");

    Ok(())
}

/// Remove the PTR record corresponding to an A record, if it matches.
pub fn delete_ptr_for_a(
    db: &Db,
    record_name: &str,
    forward_zone_name: &str,
    ip: Ipv4Addr,
) -> Result<()> {
    let rev_zone_name = reverse_zone_v4(ip);
    let rev_zone = match db.get_zone_by_name(&rev_zone_name)? {
        Some(z) => z,
        None => return Ok(()),
    };

    let ptr_name = ptr_name_v4(ip);
    let target = ptr_target(record_name, forward_zone_name);
    let ptr_data = RecordData::PTR(target);

    let existing = db.query_records(&rev_zone.id, &ptr_name, RecordType::PTR)?;
    for rec in &existing {
        if rec.data == ptr_data {
            db.delete_record(&rec.id)?;
            db.increment_soa_serial(&rev_zone.id)?;
            debug!("deleted PTR: {ptr_name}.{rev_zone_name}");
        }
    }

    Ok(())
}

/// Create or update the PTR record for an AAAA record.
/// Auto-creates the reverse zone if it doesn't exist.
pub fn sync_ptr_for_aaaa(
    db: &Db,
    record_name: &str,
    forward_zone_name: &str,
    ip: Ipv6Addr,
    ttl: u32,
) -> Result<()> {
    let rev_zone_name = reverse_zone_v6(ip);
    let rev_zone = ensure_reverse_zone(db, &rev_zone_name)?;
    let ptr_name = ptr_name_v6(ip);
    let target = ptr_target(record_name, forward_zone_name);
    let ptr_data = RecordData::PTR(target.clone());

    let existing = db.query_records(&rev_zone.id, &ptr_name, RecordType::PTR)?;
    if existing.iter().any(|r| r.data == ptr_data) {
        return Ok(());
    }

    for rec in &existing {
        db.delete_record(&rec.id)?;
    }

    let ptr_record = Record {
        id: Uuid::new_v4(),
        zone_id: rev_zone.id,
        name: ptr_name.clone(),
        ttl,
        data: ptr_data,
        enabled: true,
        health_check: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    db.create_record(&ptr_record)?;
    db.increment_soa_serial(&rev_zone.id)?;
    debug!("synced PTR: {ptr_name}.{rev_zone_name} -> {target}");

    Ok(())
}

/// Remove the PTR record corresponding to an AAAA record, if it matches.
pub fn delete_ptr_for_aaaa(
    db: &Db,
    record_name: &str,
    forward_zone_name: &str,
    ip: Ipv6Addr,
) -> Result<()> {
    let rev_zone_name = reverse_zone_v6(ip);
    let rev_zone = match db.get_zone_by_name(&rev_zone_name)? {
        Some(z) => z,
        None => return Ok(()),
    };

    let ptr_name = ptr_name_v6(ip);
    let target = ptr_target(record_name, forward_zone_name);
    let ptr_data = RecordData::PTR(target);

    let existing = db.query_records(&rev_zone.id, &ptr_name, RecordType::PTR)?;
    for rec in &existing {
        if rec.data == ptr_data {
            db.delete_record(&rec.id)?;
            db.increment_soa_serial(&rev_zone.id)?;
            debug!("deleted PTR: {ptr_name}.{rev_zone_name}");
        }
    }

    Ok(())
}

/// Sync reverse PTR for any RecordData that has an IP (A or AAAA).
/// Skips if the forward zone is itself a reverse zone.
pub fn sync_reverse_record(
    db: &Db,
    record_name: &str,
    forward_zone_name: &str,
    data: &RecordData,
    ttl: u32,
) -> Result<()> {
    if is_reverse_zone(forward_zone_name) {
        return Ok(());
    }

    match data {
        RecordData::A(ip) => sync_ptr_for_a(db, record_name, forward_zone_name, *ip, ttl),
        RecordData::AAAA(ip) => sync_ptr_for_aaaa(db, record_name, forward_zone_name, *ip, ttl),
        _ => Ok(()),
    }
}

/// Delete reverse PTR for any RecordData that has an IP.
/// Skips if the forward zone is itself a reverse zone.
pub fn delete_reverse_record(
    db: &Db,
    record_name: &str,
    forward_zone_name: &str,
    data: &RecordData,
) -> Result<()> {
    if is_reverse_zone(forward_zone_name) {
        return Ok(());
    }

    match data {
        RecordData::A(ip) => delete_ptr_for_a(db, record_name, forward_zone_name, *ip),
        RecordData::AAAA(ip) => delete_ptr_for_aaaa(db, record_name, forward_zone_name, *ip),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db() -> (Db, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Db::open(&dir.path().join("test.redb")).unwrap();
        (db, dir)
    }

    fn make_forward_zone(db: &Db, name: &str) -> Zone {
        let zone = Zone {
            id: Uuid::new_v4(),
            name: name.to_string(),
            soa: SoaData {
                mname: format!("ns1.{name}"),
                rname: format!("admin.{name}"),
                serial: 2024010100,
                refresh: 3600,
                retry: 900,
                expire: 604800,
                minimum: 300,
            },
            default_ttl: 300,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        db.create_zone(name, &zone).unwrap();
        zone
    }

    #[test]
    fn test_reverse_zone_v4() {
        assert_eq!(
            reverse_zone_v4("192.168.10.5".parse().unwrap()),
            "10.168.192.in-addr.arpa"
        );
        assert_eq!(
            reverse_zone_v4("10.0.0.1".parse().unwrap()),
            "0.0.10.in-addr.arpa"
        );
    }

    #[test]
    fn test_ptr_name_v4() {
        assert_eq!(ptr_name_v4("192.168.10.42".parse().unwrap()), "42");
        assert_eq!(ptr_name_v4("10.0.0.1".parse().unwrap()), "1");
    }

    #[test]
    fn test_reverse_zone_v6() {
        let ip: Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert_eq!(
            reverse_zone_v6(ip),
            "0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa"
        );
    }

    #[test]
    fn test_ptr_name_v6() {
        let ip: Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert_eq!(ptr_name_v6(ip), "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0");
    }

    #[test]
    fn test_is_reverse_zone() {
        assert!(is_reverse_zone("10.168.192.in-addr.arpa"));
        assert!(is_reverse_zone("0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa"));
        assert!(!is_reverse_zone("g10.lo"));
        assert!(!is_reverse_zone("example.com"));
    }

    #[test]
    fn test_ptr_target() {
        assert_eq!(ptr_target("www", "g10.lo"), "www.g10.lo.");
        assert_eq!(ptr_target("@", "g10.lo"), "g10.lo.");
        assert_eq!(ptr_target("server1", "g10.lo."), "server1.g10.lo.");
    }

    #[test]
    fn test_sync_and_delete_ptr_v4() {
        let (db, _dir) = test_db();
        make_forward_zone(&db, "g10.lo");

        let ip: Ipv4Addr = "192.168.10.5".parse().unwrap();

        // Sync creates reverse zone and PTR
        sync_ptr_for_a(&db, "server1", "g10.lo", ip, 300).unwrap();

        let rev_zone = db
            .get_zone_by_name("10.168.192.in-addr.arpa")
            .unwrap()
            .unwrap();
        let ptrs = db.query_records(&rev_zone.id, "5", RecordType::PTR).unwrap();
        assert_eq!(ptrs.len(), 1);
        assert_eq!(ptrs[0].data, RecordData::PTR("server1.g10.lo.".to_string()));

        // Idempotent — second sync doesn't duplicate
        sync_ptr_for_a(&db, "server1", "g10.lo", ip, 300).unwrap();
        let ptrs = db.query_records(&rev_zone.id, "5", RecordType::PTR).unwrap();
        assert_eq!(ptrs.len(), 1);

        // Different hostname overwrites (one IP = one PTR)
        sync_ptr_for_a(&db, "server2", "g10.lo", ip, 300).unwrap();
        let ptrs = db.query_records(&rev_zone.id, "5", RecordType::PTR).unwrap();
        assert_eq!(ptrs.len(), 1);
        assert_eq!(ptrs[0].data, RecordData::PTR("server2.g10.lo.".to_string()));

        // Delete only removes matching PTR
        delete_ptr_for_a(&db, "server2", "g10.lo", ip).unwrap();
        let ptrs = db.query_records(&rev_zone.id, "5", RecordType::PTR).unwrap();
        assert_eq!(ptrs.len(), 0);
    }

    #[test]
    fn test_sync_skips_reverse_zones() {
        let (db, _dir) = test_db();

        // Should not create PTR when zone is already a reverse zone
        let result = sync_reverse_record(
            &db,
            "10",
            "168.192.in-addr.arpa",
            &RecordData::A("192.168.10.10".parse().unwrap()),
            300,
        );
        assert!(result.is_ok());

        // No reverse zone should have been created
        assert!(db
            .get_zone_by_name("10.168.192.in-addr.arpa")
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_sync_non_address_records_noop() {
        let (db, _dir) = test_db();
        make_forward_zone(&db, "example.com");

        // CNAME should be a no-op
        let result = sync_reverse_record(
            &db,
            "www",
            "example.com",
            &RecordData::CNAME("other.example.com.".to_string()),
            300,
        );
        assert!(result.is_ok());

        // No reverse zones should exist
        assert!(db.list_zones().unwrap().len() == 1); // only forward zone
    }

    #[test]
    fn test_ensure_reverse_zone_idempotent() {
        let (db, _dir) = test_db();

        let z1 = ensure_reverse_zone(&db, "10.168.192.in-addr.arpa").unwrap();
        let z2 = ensure_reverse_zone(&db, "10.168.192.in-addr.arpa").unwrap();
        assert_eq!(z1.id, z2.id);
        assert_eq!(db.list_zones().unwrap().len(), 1);
    }

    #[test]
    fn test_sync_ptr_zone_apex() {
        let (db, _dir) = test_db();
        make_forward_zone(&db, "g10.lo");

        let ip: Ipv4Addr = "192.168.10.1".parse().unwrap();
        sync_ptr_for_a(&db, "@", "g10.lo", ip, 300).unwrap();

        let rev_zone = db
            .get_zone_by_name("10.168.192.in-addr.arpa")
            .unwrap()
            .unwrap();
        let ptrs = db.query_records(&rev_zone.id, "1", RecordType::PTR).unwrap();
        assert_eq!(ptrs.len(), 1);
        assert_eq!(ptrs[0].data, RecordData::PTR("g10.lo.".to_string()));
    }
}
