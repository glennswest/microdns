use hickory_proto::rr::rdata::{CAA, CNAME, MX, NS, PTR, SOA, SRV, TXT};
use hickory_proto::rr::{LowerName, Name, RData, Record as DnsRecord, RecordType};
use microdns_core::db::Db;
use microdns_core::types::{CaaData, RecordData, RecordType as MicroRecordType, SrvData, Zone};
use std::str::FromStr;

/// Convert our internal RecordType to hickory's RecordType
pub fn to_hickory_rtype(rt: MicroRecordType) -> RecordType {
    match rt {
        MicroRecordType::A => RecordType::A,
        MicroRecordType::AAAA => RecordType::AAAA,
        MicroRecordType::CNAME => RecordType::CNAME,
        MicroRecordType::MX => RecordType::MX,
        MicroRecordType::NS => RecordType::NS,
        MicroRecordType::PTR => RecordType::PTR,
        MicroRecordType::SOA => RecordType::SOA,
        MicroRecordType::SRV => RecordType::SRV,
        MicroRecordType::TXT => RecordType::TXT,
        MicroRecordType::CAA => RecordType::CAA,
    }
}

/// Convert hickory's RecordType to our internal type
pub fn from_hickory_rtype(rt: RecordType) -> Option<MicroRecordType> {
    match rt {
        RecordType::A => Some(MicroRecordType::A),
        RecordType::AAAA => Some(MicroRecordType::AAAA),
        RecordType::CNAME => Some(MicroRecordType::CNAME),
        RecordType::MX => Some(MicroRecordType::MX),
        RecordType::NS => Some(MicroRecordType::NS),
        RecordType::PTR => Some(MicroRecordType::PTR),
        RecordType::SOA => Some(MicroRecordType::SOA),
        RecordType::SRV => Some(MicroRecordType::SRV),
        RecordType::TXT => Some(MicroRecordType::TXT),
        RecordType::CAA => Some(MicroRecordType::CAA),
        _ => None,
    }
}

/// Convert our internal RecordData to hickory RData
pub fn to_rdata(data: &RecordData) -> Option<RData> {
    match data {
        RecordData::A(addr) => Some(RData::A((*addr).into())),
        RecordData::AAAA(addr) => Some(RData::AAAA((*addr).into())),
        RecordData::CNAME(name) => Name::from_str(&ensure_fqdn(name))
            .ok()
            .map(|n| RData::CNAME(CNAME(n))),
        RecordData::MX {
            preference,
            exchange,
        } => Name::from_str(&ensure_fqdn(exchange))
            .ok()
            .map(|name| RData::MX(MX::new(*preference, name))),
        RecordData::NS(name) => Name::from_str(&ensure_fqdn(name))
            .ok()
            .map(|n| RData::NS(NS(n))),
        RecordData::PTR(name) => Name::from_str(&ensure_fqdn(name))
            .ok()
            .map(|n| RData::PTR(PTR(n))),
        RecordData::SOA(soa) => {
            let mname = Name::from_str(&ensure_fqdn(&soa.mname)).ok()?;
            let rname = Name::from_str(&ensure_fqdn(&soa.rname)).ok()?;
            Some(RData::SOA(SOA::new(
                mname,
                rname,
                soa.serial,
                soa.refresh as i32,
                soa.retry as i32,
                soa.expire as i32,
                soa.minimum,
            )))
        }
        RecordData::SRV(srv) => {
            let target = Name::from_str(&ensure_fqdn(&srv.target)).ok()?;
            Some(RData::SRV(SRV::new(
                srv.priority,
                srv.weight,
                srv.port,
                target,
            )))
        }
        RecordData::TXT(text) => Some(RData::TXT(TXT::new(vec![text.clone()]))),
        RecordData::CAA(caa) => Some(RData::CAA(CAA::new_issue(
            caa.flags & 0x80 != 0,
            Name::from_str(&caa.value).ok(),
            vec![],
        ))),
    }
}

/// Convert hickory RData back to microdns RecordData (reverse of to_rdata).
/// Returns (relative_name, RecordData) or None for unsupported/SOA records.
pub fn from_rdata(rdata: &RData, name: &Name, zone_name: &str) -> Option<(String, RecordData)> {
    let zone_fqdn = ensure_fqdn(zone_name);
    let name_str = name.to_string();

    // Convert FQDN name to relative: "foo.gw.lo." → "foo", "gw.lo." → "@"
    let relative = if name_str == zone_fqdn || name_str.trim_end_matches('.') == zone_name {
        "@".to_string()
    } else if let Some(prefix) = name_str.strip_suffix(&format!(".{zone_fqdn}")) {
        prefix.to_string()
    } else if let Some(prefix) =
        name_str.strip_suffix(&format!(".{}.", zone_name.trim_end_matches('.')))
    {
        prefix.to_string()
    } else {
        return None;
    };

    let data = match rdata {
        RData::A(a) => RecordData::A(a.0),
        RData::AAAA(a) => RecordData::AAAA(a.0),
        RData::CNAME(CNAME(c)) => RecordData::CNAME(strip_trailing_dot(&c.to_string())),
        RData::MX(mx) => RecordData::MX {
            preference: mx.preference(),
            exchange: strip_trailing_dot(&mx.exchange().to_string()),
        },
        RData::NS(NS(ns)) => RecordData::NS(strip_trailing_dot(&ns.to_string())),
        RData::PTR(PTR(ptr)) => RecordData::PTR(strip_trailing_dot(&ptr.to_string())),
        RData::SRV(srv) => RecordData::SRV(SrvData {
            priority: srv.priority(),
            weight: srv.weight(),
            port: srv.port(),
            target: strip_trailing_dot(&srv.target().to_string()),
        }),
        RData::TXT(txt) => {
            let joined: String = txt
                .txt_data()
                .iter()
                .map(|b| String::from_utf8_lossy(b).to_string())
                .collect::<Vec<_>>()
                .join("");
            RecordData::TXT(joined)
        }
        RData::CAA(caa) => RecordData::CAA(CaaData {
            flags: if caa.issuer_critical() { 0x80 } else { 0 },
            tag: caa.tag().to_string(),
            value: caa.value().to_string(),
        }),
        RData::SOA(_) => return None,
        _ => return None,
    };

    Some((relative, data))
}

fn strip_trailing_dot(s: &str) -> String {
    s.strip_suffix('.').unwrap_or(s).to_string()
}

fn ensure_fqdn(name: &str) -> String {
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{name}.")
    }
}

/// Build the SOA record for a zone
pub fn build_soa_record(zone: &Zone) -> Option<DnsRecord> {
    let zone_name = Name::from_str(&ensure_fqdn(&zone.name)).ok()?;
    let soa_data = RecordData::SOA(zone.soa.clone());
    let rdata = to_rdata(&soa_data)?;

    let mut record = DnsRecord::from_rdata(zone_name, zone.default_ttl, rdata);
    record.set_record_type(RecordType::SOA);
    Some(record)
}

/// Resolve a query against the database
pub fn resolve_query(db: &Db, qname: &LowerName, qtype: RecordType) -> Vec<DnsRecord> {
    let fqdn = qname.to_string();
    let fqdn = fqdn.trim_end_matches('.');

    // Handle SOA queries
    if qtype == RecordType::SOA {
        if let Ok(Some(zone)) = db.find_zone_for_fqdn(fqdn) {
            if let Some(soa) = build_soa_record(&zone) {
                return vec![soa];
            }
        }
        return Vec::new();
    }

    // Convert hickory type to our type
    let micro_rtype = match from_hickory_rtype(qtype) {
        Some(rt) => rt,
        None => return Vec::new(),
    };

    // CoreDNS-style wildcard query: a query that itself contains a `*` label
    // (e.g. `*.default.svc.cluster.local`, or `_http._tcp.*.ns.svc...`) matches
    // every record at that position and returns them under their real names.
    // Normal queries never contain `*`, so this path only ever *adds* answers
    // for explicit wildcard queries and cannot affect ordinary resolution.
    if fqdn.split('.').any(|label| label == "*") {
        return resolve_wildcard_query(db, fqdn, micro_rtype);
    }

    // Query the database
    let records = match db.query_fqdn(fqdn, micro_rtype) {
        Ok(records) => records,
        Err(e) => {
            tracing::error!("failed to query records for {fqdn}/{qtype}: {e}");
            return Vec::new();
        }
    };

    let mut dns_records = Vec::new();
    for record in &records {
        let name = if record.name == "@" {
            match db.find_zone_for_fqdn(fqdn) {
                Ok(Some(zone)) => match Name::from_str(&ensure_fqdn(&zone.name)) {
                    Ok(n) => n,
                    Err(_) => continue,
                },
                _ => continue,
            }
        } else if record.name.starts_with('*') {
            // Wildcard match: respond with the queried FQDN, not *.zone
            match Name::from_str(&ensure_fqdn(fqdn)) {
                Ok(n) => n,
                Err(_) => continue,
            }
        } else {
            match db.find_zone_for_fqdn(fqdn) {
                Ok(Some(zone)) => {
                    match Name::from_str(&format!("{}.{}.", record.name, zone.name)) {
                        Ok(n) => n,
                        Err(_) => continue,
                    }
                }
                _ => continue,
            }
        };

        if let Some(rdata) = to_rdata(&record.data) {
            let dns_record = DnsRecord::from_rdata(name, record.ttl, rdata);
            dns_records.push(dns_record);
        }
    }

    dns_records
}

/// Resolve a query whose name contains one or more `*` labels by matching every
/// record in the owning zone where `*` stands for any single label (CoreDNS
/// behavior). Answers are returned under each record's real FQDN.
fn resolve_wildcard_query(db: &Db, fqdn: &str, rtype: MicroRecordType) -> Vec<DnsRecord> {
    let zone = match db.find_zone_for_fqdn(fqdn) {
        Ok(Some(z)) => z,
        _ => return Vec::new(),
    };
    let zone_name = zone.name.trim_end_matches('.');

    // Query pattern relative to the zone origin, e.g. `*.default.svc`.
    let pattern = match fqdn.strip_suffix(&format!(".{zone_name}")) {
        Some(p) => p,
        None => return Vec::new(),
    };
    let pat_labels: Vec<&str> = pattern.split('.').collect();

    let records = match db.list_records(&zone.id) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for record in &records {
        if !record.enabled || record.data.record_type() != rtype {
            continue;
        }
        let rec_labels: Vec<&str> = record.name.split('.').collect();
        if rec_labels.len() != pat_labels.len() {
            continue;
        }
        // `*` in the query matches any single label; other labels must be equal.
        let matches = pat_labels
            .iter()
            .zip(&rec_labels)
            .all(|(p, r)| *p == "*" || p == r);
        if !matches {
            continue;
        }
        let name = match Name::from_str(&format!("{}.{}.", record.name, zone_name)) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if let Some(rdata) = to_rdata(&record.data) {
            out.push(DnsRecord::from_rdata(name, record.ttl, rdata));
        }
    }
    out
}

/// Get the SOA record for the authority section (NXDOMAIN responses)
pub fn get_authority_soa(db: &Db, qname: &LowerName) -> Option<DnsRecord> {
    let fqdn = qname.to_string();
    let fqdn = fqdn.trim_end_matches('.');

    db.find_zone_for_fqdn(fqdn)
        .ok()
        .flatten()
        .and_then(|zone| build_soa_record(&zone))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use microdns_core::db::Db;
    use microdns_core::types::{Record, RecordData as RD, SoaData, Zone};
    use uuid::Uuid;

    fn soa() -> SoaData {
        SoaData {
            mname: "ns.cluster.local".into(),
            rname: "admin.cluster.local".into(),
            serial: 1,
            refresh: 3600,
            retry: 900,
            expire: 604800,
            minimum: 30,
        }
    }

    fn add_a(db: &Db, zone: &Uuid, name: &str, ip: &str) {
        db.create_record(&Record {
            id: Uuid::new_v4(),
            zone_id: *zone,
            name: name.into(),
            ttl: 30,
            data: RD::A(ip.parse().unwrap()),
            enabled: true,
            health_check: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();
    }

    #[test]
    fn wildcard_query_returns_all_matching_and_leaves_normal_queries_intact() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.redb")).unwrap();
        let zid = Uuid::new_v4();
        db.create_zone(
            "cluster.local",
            &Zone {
                id: zid,
                name: "cluster.local".into(),
                soa: soa(),
                default_ttl: 30,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
        )
        .unwrap();
        add_a(&db, &zid, "a.default.svc", "10.0.0.1");
        add_a(&db, &zid, "b.default.svc", "10.0.0.2");
        add_a(&db, &zid, "c.other.svc", "10.0.0.3");

        // CoreDNS-style wildcard: `*.default.svc` → a + b (not c in `other`).
        let q = LowerName::from(Name::from_str("*.default.svc.cluster.local.").unwrap());
        assert_eq!(resolve_query(&db, &q, RecordType::A).len(), 2);

        // A normal (non-`*`) query still resolves exactly one record.
        let q2 = LowerName::from(Name::from_str("a.default.svc.cluster.local.").unwrap());
        assert_eq!(resolve_query(&db, &q2, RecordType::A).len(), 1);

        // A non-existent normal name still returns nothing (NXDOMAIN path).
        let q3 = LowerName::from(Name::from_str("nope.default.svc.cluster.local.").unwrap());
        assert_eq!(resolve_query(&db, &q3, RecordType::A).len(), 0);
    }
}
