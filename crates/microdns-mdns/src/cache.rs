//! The learned-record cache.
//!
//! Everything the listener hears lands here first, with the announced (clamped)
//! TTL as its lifetime. The cache — not the wire — is what gets reconciled into
//! the zone, which means a device that stops announcing disappears from DNS on
//! its own schedule rather than lingering until someone notices.
//!
//! Cache maintenance follows RFC 6762 §5.2: a record that is about to expire is
//! re-queried rather than dropped, so a device that is still on the network
//! stays published indefinitely without ever re-announcing spontaneously.

use std::collections::HashMap;
use std::net::IpAddr;

use chrono::{DateTime, Duration, Utc};
use microdns_core::types::{RecordData, RecordType};

/// Re-query once a record has used up this fraction of its lifetime.
const REFRESH_AT: f64 = 0.8;

/// The DNS-SD meta-query whose answers are service *types*, not instances
/// (RFC 6763 §9).
pub const SERVICE_ENUMERATION: &str = "_services._dns-sd._udp.local";

/// One learned mDNS record.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Owner name as announced, lowercased, without the trailing dot —
    /// e.g. `teslatracker-52c4.local`.
    pub name: String,
    pub data: RecordData,
    /// TTL after clamping, in seconds. Also the entry's lifetime.
    pub ttl: u32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Address the announcement came from, for operator visibility.
    pub from: IpAddr,
    /// Whether a maintenance re-query has already gone out for this lifetime.
    /// Reset every time the record is re-announced.
    pub(crate) refresh_sent: bool,
}

impl Entry {
    pub fn record_type(&self) -> RecordType {
        self.data.record_type()
    }

    /// Seconds until this entry falls out of the cache (0 once due).
    pub fn expires_in(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_seconds().max(0)
    }
}

/// What `learn` did, so the caller knows whether the zone needs rewriting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Learned {
    /// A record we had not seen before.
    New,
    /// A record we already had, with its lifetime extended.
    Refreshed,
    /// A goodbye (TTL 0) that removed a record we held.
    Removed,
    /// A goodbye for something we never had.
    Ignored,
}

impl Learned {
    /// Whether this changed the published set.
    pub fn changes_zone(&self) -> bool {
        matches!(self, Learned::New | Learned::Removed)
    }
}

/// Running counters, surfaced through the REST API.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub packets: u64,
    pub records_learned: u64,
    pub goodbyes: u64,
    pub expired: u64,
    pub queries_sent: u64,
}

#[derive(Debug, Default)]
pub struct MdnsCache {
    /// Keyed by owner name and type; the vec holds one entry per distinct
    /// rdata, which is how a multi-homed host ends up with two A records.
    entries: HashMap<(String, RecordType), Vec<Entry>>,
    pub stats: Stats,
    pub last_packet_at: Option<DateTime<Utc>>,
}

impl MdnsCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an announcement. `ttl` is expected to be clamped already; a TTL
    /// of 0 is a goodbye and removes the matching entry (RFC 6762 §10.1).
    pub fn learn(
        &mut self,
        name: &str,
        data: RecordData,
        ttl: u32,
        from: IpAddr,
        now: DateTime<Utc>,
    ) -> Learned {
        let key = (name.to_lowercase(), data.record_type());

        if ttl == 0 {
            self.stats.goodbyes += 1;
            let Some(list) = self.entries.get_mut(&key) else {
                return Learned::Ignored;
            };
            let before = list.len();
            list.retain(|e| e.data != data);
            let removed = list.len() < before;
            let emptied = list.is_empty();
            if emptied {
                self.entries.remove(&key);
            }
            return if removed {
                Learned::Removed
            } else {
                Learned::Ignored
            };
        }

        let expires_at = now + Duration::seconds(i64::from(ttl));
        let list = self.entries.entry(key).or_default();

        if let Some(existing) = list.iter_mut().find(|e| e.data == data) {
            existing.ttl = ttl;
            existing.last_seen = now;
            existing.expires_at = expires_at;
            existing.from = from;
            existing.refresh_sent = false;
            return Learned::Refreshed;
        }

        list.push(Entry {
            name: name.to_lowercase(),
            data,
            ttl,
            first_seen: now,
            last_seen: now,
            expires_at,
            from,
            refresh_sent: false,
        });
        self.stats.records_learned += 1;
        Learned::New
    }

    /// Drop everything past its lifetime. Returns how many entries went.
    pub fn expire(&mut self, now: DateTime<Utc>) -> usize {
        let mut removed = 0;
        self.entries.retain(|_, list| {
            let before = list.len();
            list.retain(|e| e.expires_at > now);
            removed += before - list.len();
            !list.is_empty()
        });
        self.stats.expired += removed as u64;
        removed
    }

    /// Names due for a maintenance re-query — those past `REFRESH_AT` of their
    /// lifetime that have not been asked about yet. Marks them as asked, so a
    /// caller that drops the result still will not re-query in a tight loop.
    pub fn take_refresh_due(&mut self, now: DateTime<Utc>) -> Vec<(String, RecordType)> {
        let mut due = Vec::new();
        for ((name, rtype), list) in self.entries.iter_mut() {
            let mut any = false;
            for entry in list.iter_mut() {
                if entry.refresh_sent {
                    continue;
                }
                let lifetime = f64::from(entry.ttl);
                let elapsed = (now - entry.last_seen).num_seconds() as f64;
                if elapsed >= lifetime * REFRESH_AT {
                    entry.refresh_sent = true;
                    any = true;
                }
            }
            if any {
                due.push((name.clone(), *rtype));
            }
        }
        due
    }

    /// Forget everything. Used when the source stops listening: a cache held
    /// across a stop would claim devices are present that nobody has heard
    /// from since.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.last_packet_at = None;
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values().flatten()
    }

    pub fn len(&self) -> usize {
        self.entries.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Distinct DNS-SD service types currently known (`_ipp._tcp.local`), used
    /// to aim the periodic enumeration queries.
    ///
    /// Two things name a service type. The meta-query
    /// `_services._dns-sd._udp.local` answers with one PTR *per type*, so its
    /// targets are types — that is how a browse learns what exists at all.
    /// A type also appears as the owner of the PTRs listing its instances, which
    /// is how types learned from a spontaneous announcement show up.
    pub fn service_types(&self) -> Vec<String> {
        let mut types: Vec<String> = Vec::new();

        for ((name, rtype), list) in &self.entries {
            if *rtype != RecordType::PTR {
                continue;
            }
            if name == SERVICE_ENUMERATION {
                types.extend(list.iter().filter_map(|e| match &e.data {
                    RecordData::PTR(target) => Some(target.trim_end_matches('.').to_lowercase()),
                    _ => None,
                }));
            } else if name.starts_with('_') {
                types.push(name.clone());
            }
        }

        types.sort();
        types.dedup();
        types
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn a(s: &str) -> RecordData {
        RecordData::A(s.parse::<Ipv4Addr>().unwrap())
    }

    #[test]
    fn learning_then_re_announcing_extends_the_lifetime() {
        let mut cache = MdnsCache::new();
        let t0 = Utc::now();

        assert_eq!(
            cache.learn("host.local", a("192.168.9.134"), 120, ip("192.168.9.134"), t0),
            Learned::New
        );
        assert_eq!(cache.len(), 1);

        let t1 = t0 + Duration::seconds(60);
        assert_eq!(
            cache.learn("host.local", a("192.168.9.134"), 120, ip("192.168.9.134"), t1),
            Learned::Refreshed
        );
        assert_eq!(cache.len(), 1);

        // The re-announcement pushed expiry out; the entry survives the point
        // the first announcement would have expired at.
        assert_eq!(cache.expire(t0 + Duration::seconds(121)), 0);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.expire(t1 + Duration::seconds(121)), 1);
        assert!(cache.is_empty());
    }

    #[test]
    fn a_second_address_for_one_name_is_kept_alongside_the_first() {
        let mut cache = MdnsCache::new();
        let t0 = Utc::now();
        cache.learn("host.local", a("192.168.9.134"), 120, ip("192.168.9.134"), t0);
        cache.learn("host.local", a("192.168.9.135"), 120, ip("192.168.9.135"), t0);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn goodbye_removes_only_the_announced_rdata() {
        let mut cache = MdnsCache::new();
        let t0 = Utc::now();
        cache.learn("host.local", a("192.168.9.134"), 120, ip("192.168.9.134"), t0);
        cache.learn("host.local", a("192.168.9.135"), 120, ip("192.168.9.135"), t0);

        assert_eq!(
            cache.learn("host.local", a("192.168.9.134"), 0, ip("192.168.9.134"), t0),
            Learned::Removed
        );
        assert_eq!(cache.len(), 1);

        // A goodbye for something we never held changes nothing.
        assert_eq!(
            cache.learn("other.local", a("192.168.9.9"), 0, ip("192.168.9.9"), t0),
            Learned::Ignored
        );
    }

    #[test]
    fn refresh_is_due_at_eighty_percent_and_only_asked_once() {
        let mut cache = MdnsCache::new();
        let t0 = Utc::now();
        cache.learn("host.local", a("192.168.9.134"), 100, ip("192.168.9.134"), t0);

        assert!(cache.take_refresh_due(t0 + Duration::seconds(70)).is_empty());

        let due = cache.take_refresh_due(t0 + Duration::seconds(85));
        assert_eq!(due, vec![("host.local".to_string(), RecordType::A)]);

        // Asking again before the record is re-announced must not re-query.
        assert!(cache.take_refresh_due(t0 + Duration::seconds(90)).is_empty());

        // A fresh announcement re-arms it.
        cache.learn(
            "host.local",
            a("192.168.9.134"),
            100,
            ip("192.168.9.134"),
            t0 + Duration::seconds(90),
        );
        assert_eq!(cache.take_refresh_due(t0 + Duration::seconds(175)).len(), 1);
    }

    #[test]
    fn service_types_come_from_ptr_owners_and_meta_query_targets() {
        let mut cache = MdnsCache::new();
        let t0 = Utc::now();

        // A type learned from an instance listing.
        cache.learn(
            "_ipp._tcp.local",
            RecordData::PTR("printer._ipp._tcp.local.".into()),
            4500,
            ip("192.168.9.2"),
            t0,
        );
        // Types learned from the meta-query: the *targets* name the types, so
        // browsing them is what turns an enumeration into actual instances.
        cache.learn(
            SERVICE_ENUMERATION,
            RecordData::PTR("_airplay._tcp.local.".into()),
            4500,
            ip("192.168.9.3"),
            t0,
        );
        cache.learn(
            SERVICE_ENUMERATION,
            RecordData::PTR("_ssh._tcp.local.".into()),
            4500,
            ip("192.168.9.3"),
            t0,
        );
        // Plain hosts are not service types.
        cache.learn("host.local", a("192.168.9.134"), 120, ip("192.168.9.134"), t0);

        assert_eq!(
            cache.service_types(),
            vec![
                "_airplay._tcp.local".to_string(),
                "_ipp._tcp.local".to_string(),
                "_ssh._tcp.local".to_string(),
            ]
        );
        assert!(
            !cache.service_types().contains(&SERVICE_ENUMERATION.to_string()),
            "the meta-query is not itself a service type to browse"
        );
    }
}
