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

/// Get the SOA record for the authority section (NXDOMAIN responses)
pub fn get_authority_soa(db: &Db, qname: &LowerName) -> Option<DnsRecord> {
    let fqdn = qname.to_string();
    let fqdn = fqdn.trim_end_matches('.');

    db.find_zone_for_fqdn(fqdn)
        .ok()
        .flatten()
        .and_then(|zone| build_soa_record(&zone))
}
