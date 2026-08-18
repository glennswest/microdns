//! Map learned `.local` names onto records in the publish zone.
//!
//! The mapping is a straight relabel: `teslatracker-52c4.local` announced on
//! the segment becomes `teslatracker-52c4` in, say, `mdns.g9.lo`. Names inside
//! rdata (a PTR's target, an SRV's host) are relabelled the same way, so a
//! DNS-SD browse that starts in the zone stays in the zone instead of walking
//! back out to `.local` — which is exactly the namespace the querying client
//! could not reach in the first place.

use microdns_core::types::{RecordData, RecordType, SrvData};

use crate::cache::Entry;
use crate::config::MdnsConfig;

/// A record the source wants to exist in the publish zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredRecord {
    /// Name relative to the publish zone.
    pub name: String,
    pub ttl: u32,
    pub data: RecordData,
}

impl DesiredRecord {
    pub fn record_type(&self) -> RecordType {
        self.data.record_type()
    }
}

/// Build the desired record set from the cache contents.
pub fn desired(entries: impl Iterator<Item = Entry>, config: &MdnsConfig) -> Vec<DesiredRecord> {
    let mut out: Vec<DesiredRecord> = Vec::new();
    for entry in entries {
        if let Some(record) = translate(&entry, config) {
            if !out.contains(&record) {
                out.push(record);
            }
        }
    }
    out.sort_by(|a, b| (&a.name, a.record_type().to_string()).cmp(&(&b.name, b.record_type().to_string())));
    out
}

/// Translate one cache entry, or `None` if config says not to publish it.
pub fn translate(entry: &Entry, config: &MdnsConfig) -> Option<DesiredRecord> {
    let rtype = entry.record_type();
    if !config.services && matches!(rtype, RecordType::PTR | RecordType::SRV | RecordType::TXT) {
        return None;
    }

    let name = relative_name(&entry.name)?;
    if !config.permits(&name) {
        return None;
    }

    let data = match &entry.data {
        RecordData::PTR(target) => RecordData::PTR(rewrite_target(target, &config.zone)),
        RecordData::SRV(srv) => RecordData::SRV(SrvData {
            target: rewrite_target(&srv.target, &config.zone),
            ..srv.clone()
        }),
        other => other.clone(),
    };

    Some(DesiredRecord {
        name,
        ttl: config.clamp_ttl(entry.ttl),
        data,
    })
}

/// Strip the `.local` suffix to get a name relative to the publish zone.
/// The `.local` apex itself has nothing to publish.
pub fn relative_name(mdns_name: &str) -> Option<String> {
    let name = mdns_name.trim_end_matches('.').to_lowercase();
    let stripped = name.strip_suffix(".local")?;
    if stripped.is_empty() {
        return None;
    }
    Some(stripped.to_string())
}

/// Rewrite a `.local` target into the publish zone, leaving anything else
/// alone. Targets keep the trailing dot other microdns records use.
pub fn rewrite_target(target: &str, zone: &str) -> String {
    let zone = zone.trim_end_matches('.');
    match relative_name(target) {
        Some(rel) => format!("{rel}.{zone}."),
        None => target.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use std::net::IpAddr;

    fn entry(name: &str, data: RecordData, ttl: u32) -> Entry {
        let now = Utc::now();
        Entry {
            name: name.to_string(),
            data,
            ttl,
            first_seen: now,
            last_seen: now,
            expires_at: now + Duration::seconds(i64::from(ttl)),
            from: "192.168.9.134".parse::<IpAddr>().unwrap(),
            refresh_sent: false,
        }
    }

    fn config() -> MdnsConfig {
        MdnsConfig {
            zone: "mdns.g9.lo".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_host_becomes_a_name_in_the_publish_zone() {
        let e = entry(
            "teslatracker-52c4.local",
            RecordData::A("192.168.9.134".parse().unwrap()),
            120,
        );
        let got = translate(&e, &config()).unwrap();
        assert_eq!(got.name, "teslatracker-52c4");
        assert_eq!(got.ttl, 120);
        assert_eq!(got.data, RecordData::A("192.168.9.134".parse().unwrap()));
    }

    #[test]
    fn ptr_and_srv_targets_are_pulled_into_the_publish_zone() {
        let cfg = config();

        let ptr = entry(
            "_http._tcp.local",
            RecordData::PTR("tracker._http._tcp.local.".into()),
            4500,
        );
        let got = translate(&ptr, &cfg).unwrap();
        assert_eq!(got.name, "_http._tcp");
        assert_eq!(
            got.data,
            RecordData::PTR("tracker._http._tcp.mdns.g9.lo.".into())
        );

        let srv = entry(
            "tracker._http._tcp.local",
            RecordData::SRV(SrvData {
                priority: 0,
                weight: 0,
                port: 8080,
                target: "teslatracker-52c4.local.".into(),
            }),
            120,
        );
        match translate(&srv, &cfg).unwrap().data {
            RecordData::SRV(srv) => {
                assert_eq!(srv.target, "teslatracker-52c4.mdns.g9.lo.");
                assert_eq!(srv.port, 8080);
            }
            other => panic!("expected SRV, got {other:?}"),
        }
    }

    #[test]
    fn a_target_outside_local_is_left_untouched() {
        assert_eq!(
            rewrite_target("host.example.com.", "mdns.g9.lo"),
            "host.example.com."
        );
    }

    #[test]
    fn ttl_is_clamped_on_the_way_out() {
        let cfg = MdnsConfig {
            ttl_max: 300,
            ..config()
        };
        let e = entry(
            "printer._ipp._tcp.local",
            RecordData::PTR("x._ipp._tcp.local.".into()),
            4500,
        );
        assert_eq!(translate(&e, &cfg).unwrap().ttl, 300);
    }

    #[test]
    fn service_records_are_dropped_when_services_are_off() {
        let cfg = MdnsConfig {
            services: false,
            ..config()
        };
        let ptr = entry(
            "_http._tcp.local",
            RecordData::PTR("tracker._http._tcp.local.".into()),
            4500,
        );
        assert!(translate(&ptr, &cfg).is_none());

        // Addresses still come through.
        let a = entry(
            "tracker.local",
            RecordData::A("192.168.9.134".parse().unwrap()),
            120,
        );
        assert!(translate(&a, &cfg).is_some());
    }

    #[test]
    fn denied_names_are_not_published() {
        let cfg = MdnsConfig {
            deny: vec!["chromecast-*".into()],
            ..config()
        };
        let denied = entry(
            "chromecast-9911.local",
            RecordData::A("192.168.9.50".parse().unwrap()),
            120,
        );
        assert!(translate(&denied, &cfg).is_none());
    }

    #[test]
    fn the_local_apex_has_nothing_to_publish() {
        assert_eq!(relative_name("local"), None);
        assert_eq!(relative_name("local."), None);
        assert_eq!(relative_name("host.example.com"), None);
        assert_eq!(relative_name("Host.LOCAL."), Some("host".to_string()));
    }

    #[test]
    fn identical_announcements_collapse_to_one_desired_record() {
        let e = entry(
            "tracker.local",
            RecordData::A("192.168.9.134".parse().unwrap()),
            120,
        );
        let got = desired(vec![e.clone(), e].into_iter(), &config());
        assert_eq!(got.len(), 1);
    }
}
