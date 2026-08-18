//! Turn an mDNS packet into records worth learning.
//!
//! mDNS is DNS on the wire, so hickory parses it — with two wrinkles this
//! module handles. The top bit of the class field is the cache-flush bit
//! (RFC 6762 §10.2), so a class of `0x8001` is still IN and must be masked
//! before it is compared. And only *responses* carry data: a query's authority
//! section holds records a host is merely proposing to claim (§8.1), and its
//! answer section holds known-answer suppression, neither of which is an
//! assertion that the record exists.

use std::net::IpAddr;

use hickory_proto::op::{Message, MessageType};
use hickory_proto::rr::{DNSClass, RData, Record as ProtoRecord};
use microdns_core::types::{RecordData, SrvData};

/// One record lifted off the wire, ready for the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Owner name, lowercased, no trailing dot (`teslatracker-52c4.local`).
    pub name: String,
    pub data: RecordData,
    /// Announced TTL, unclamped. Zero means goodbye.
    pub ttl: u32,
}

/// Extract everything learnable from a packet.
///
/// Returns empty for queries — see the module note on why a query's records are
/// not assertions.
pub fn announcements(msg: &Message, _from: IpAddr) -> Vec<Announcement> {
    if msg.message_type() != MessageType::Response {
        return Vec::new();
    }

    msg.answers()
        .iter()
        .chain(msg.additionals())
        .filter_map(announcement)
        .collect()
}

/// Convert one wire record, dropping anything that is not a `.local` record of
/// a type we can publish.
fn announcement(record: &ProtoRecord) -> Option<Announcement> {
    if !is_internet_class(record.dns_class()) {
        return None;
    }

    let name = normalize_name(&record.name().to_string());
    if !is_local(&name) {
        return None;
    }

    let data = record_data(record.data()?)?;
    Some(Announcement {
        name,
        data,
        ttl: record.ttl(),
    })
}

/// IN, with the cache-flush bit masked off.
fn is_internet_class(class: DNSClass) -> bool {
    match class {
        DNSClass::IN => true,
        DNSClass::Unknown(raw) => raw & 0x7fff == u16::from(DNSClass::IN),
        _ => false,
    }
}

/// Lowercase and strip the trailing root dot, so names compare as written.
pub fn normalize_name(name: &str) -> String {
    name.trim_end_matches('.').to_lowercase()
}

/// Whether a name sits under the mDNS `.local` namespace.
pub fn is_local(name: &str) -> bool {
    name == "local" || name.ends_with(".local")
}

/// Map hickory rdata onto our stored form, dropping types we do not serve and
/// addresses that mean nothing off-segment.
fn record_data(rdata: &RData) -> Option<RecordData> {
    match rdata {
        RData::A(a) => {
            let ip = a.0;
            // A link-local or loopback address is only meaningful on the
            // segment it was announced on — publishing it in unicast DNS would
            // hand cross-subnet clients an address that cannot work.
            if ip.is_link_local() || ip.is_loopback() || ip.is_unspecified() || ip.is_broadcast() {
                return None;
            }
            Some(RecordData::A(ip))
        }
        RData::AAAA(aaaa) => {
            let ip = aaaa.0;
            if is_unicast_link_local(&ip) || ip.is_loopback() || ip.is_unspecified() {
                return None;
            }
            Some(RecordData::AAAA(ip))
        }
        RData::PTR(ptr) => Some(RecordData::PTR(fqdn(&ptr.0.to_string()))),
        RData::SRV(srv) => Some(RecordData::SRV(SrvData {
            priority: srv.priority(),
            weight: srv.weight(),
            port: srv.port(),
            target: fqdn(&srv.target().to_string()),
        })),
        RData::TXT(txt) => {
            // DNS-SD packs one `key=value` per character-string. Our record
            // model holds a single string, so the strings are joined the way
            // dig renders them; keys stay readable and parseable by a client
            // that splits on whitespace.
            let joined = txt
                .txt_data()
                .iter()
                .map(|d| String::from_utf8_lossy(d).to_string())
                .collect::<Vec<_>>()
                .join(" ");
            if joined.is_empty() {
                return None;
            }
            Some(RecordData::TXT(joined))
        }
        _ => None,
    }
}

/// `fe80::/10`, the IPv6 equivalent of the IPv4 link-local check above.
/// `Ipv6Addr::is_unicast_link_local` is still unstable, so it is spelled out.
fn is_unicast_link_local(ip: &std::net::Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

/// Normalise a target name to lowercase with a trailing dot, the form the rest
/// of microdns stores PTR and SRV targets in.
fn fqdn(name: &str) -> String {
    let trimmed = normalize_name(name);
    format!("{trimmed}.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{Header, MessageType, OpCode};
    use hickory_proto::rr::rdata::{A, AAAA, SRV, TXT};
    use hickory_proto::rr::{Name, RecordType as ProtoType};
    use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
    use std::str::FromStr;

    fn response(records: Vec<ProtoRecord>) -> Message {
        let mut msg = Message::new();
        let mut header = Header::new();
        header.set_message_type(MessageType::Response);
        header.set_op_code(OpCode::Query);
        header.set_authoritative(true);
        msg.set_header(header);
        for r in records {
            msg.add_answer(r);
        }
        msg
    }

    fn rec(name: &str, rdata: RData, ttl: u32) -> ProtoRecord {
        let mut r = ProtoRecord::new();
        r.set_name(Name::from_str(name).unwrap());
        r.set_ttl(ttl);
        r.set_record_type(rdata.record_type());
        r.set_data(Some(rdata));
        r
    }

    fn from(ip: &str) -> IpAddr {
        ip.parse().unwrap()
    }

    #[test]
    fn learns_an_address_announcement() {
        let msg = response(vec![rec(
            "teslatracker-52C4.local.",
            RData::A(A::new(192, 168, 9, 134)),
            120,
        )]);
        let got = announcements(&msg, from("192.168.9.134"));
        assert_eq!(
            got,
            vec![Announcement {
                name: "teslatracker-52c4.local".into(),
                data: RecordData::A("192.168.9.134".parse().unwrap()),
                ttl: 120,
            }]
        );
    }

    #[test]
    fn queries_teach_us_nothing() {
        let mut msg = Message::new();
        msg.set_message_type(MessageType::Query);
        msg.add_answer(rec(
            "host.local.",
            RData::A(A::new(192, 168, 9, 134)),
            120,
        ));
        assert!(announcements(&msg, from("192.168.9.134")).is_empty());
    }

    #[test]
    fn the_cache_flush_bit_does_not_hide_a_record() {
        // Class 0x8001 — IN with the cache-flush bit set, which is what a
        // real announcement carries. Round-trip through the wire so the class
        // is parsed exactly as a responder would have encoded it.
        let mut r = rec("host.local.", RData::A(A::new(192, 168, 9, 134)), 120);
        r.set_dns_class(DNSClass::Unknown(0x8001));
        let wire = response(vec![r]).to_bytes().unwrap();
        let parsed = Message::from_bytes(&wire).unwrap();

        let got = announcements(&parsed, from("192.168.9.134"));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].data, RecordData::A("192.168.9.134".parse().unwrap()));
    }

    #[test]
    fn goodbye_keeps_its_zero_ttl() {
        let msg = response(vec![rec(
            "host.local.",
            RData::A(A::new(192, 168, 9, 134)),
            0,
        )]);
        assert_eq!(announcements(&msg, from("192.168.9.134"))[0].ttl, 0);
    }

    #[test]
    fn link_local_and_loopback_addresses_are_dropped() {
        let msg = response(vec![
            rec("host.local.", RData::A(A::new(169, 254, 1, 2)), 120),
            rec("host.local.", RData::A(A::new(127, 0, 0, 1)), 120),
            rec(
                "host.local.",
                RData::AAAA(AAAA::from_str("fe80::1").unwrap()),
                120,
            ),
            rec(
                "host.local.",
                RData::AAAA(AAAA::from_str("2001:db8::1").unwrap()),
                120,
            ),
        ]);
        let got = announcements(&msg, from("192.168.9.134"));
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].data,
            RecordData::AAAA("2001:db8::1".parse().unwrap())
        );
    }

    #[test]
    fn names_outside_local_are_ignored() {
        let msg = response(vec![rec(
            "host.example.com.",
            RData::A(A::new(93, 184, 216, 34)),
            120,
        )]);
        assert!(announcements(&msg, from("192.168.9.134")).is_empty());
    }

    #[test]
    fn service_records_carry_over_with_normalised_targets() {
        let srv = SRV::new(0, 0, 8080, Name::from_str("Host.local.").unwrap());
        let msg = response(vec![
            rec(
                "_http._tcp.local.",
                RData::PTR(hickory_proto::rr::rdata::PTR(
                    Name::from_str("tracker._http._tcp.local.").unwrap(),
                )),
                4500,
            ),
            rec("tracker._http._tcp.local.", RData::SRV(srv), 120),
            rec(
                "tracker._http._tcp.local.",
                RData::TXT(TXT::new(vec!["path=/status".into(), "v=1".into()])),
                4500,
            ),
        ]);

        let got = announcements(&msg, from("192.168.9.134"));
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0].data,
            RecordData::PTR("tracker._http._tcp.local.".into())
        );
        match &got[1].data {
            RecordData::SRV(srv) => {
                assert_eq!(srv.port, 8080);
                assert_eq!(srv.target, "host.local.");
            }
            other => panic!("expected SRV, got {other:?}"),
        }
        assert_eq!(got[2].data, RecordData::TXT("path=/status v=1".into()));
    }

    #[test]
    fn additionals_are_learned_too() {
        // A responder answering a PTR query puts the SRV/TXT/A it knows in the
        // additional section; dropping those would mean a second round trip
        // for data already in hand.
        let mut msg = response(vec![rec(
            "_http._tcp.local.",
            RData::PTR(hickory_proto::rr::rdata::PTR(
                Name::from_str("tracker._http._tcp.local.").unwrap(),
            )),
            4500,
        )]);
        msg.add_additional(rec(
            "tracker.local.",
            RData::A(A::new(192, 168, 9, 134)),
            120,
        ));

        let got = announcements(&msg, from("192.168.9.134"));
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].name, "tracker.local");
    }

    #[test]
    fn unsupported_types_are_skipped_without_dropping_the_packet() {
        let mut msg = response(vec![rec(
            "host.local.",
            RData::A(A::new(192, 168, 9, 134)),
            120,
        )]);
        let mut nsec = ProtoRecord::new();
        nsec.set_name(Name::from_str("host.local.").unwrap());
        nsec.set_record_type(ProtoType::NSEC);
        nsec.set_ttl(120);
        msg.add_answer(nsec);

        assert_eq!(announcements(&msg, from("192.168.9.134")).len(), 1);
    }
}
