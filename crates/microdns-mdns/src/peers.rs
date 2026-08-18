//! Mirror what sibling instances can hear.
//!
//! mDNS is only audible on the segment it was announced on, so each instance
//! hears a different slice of the network. Publishing those slices into
//! per-network zones would make a name's *location* part of its address —
//! callers would have to know a device sits on g9 to ask for it.
//!
//! Instead every instance publishes into one shared zone and mirrors what its
//! siblings heard, so the same flat name resolves the same way from anywhere.
//! Each instance ends up holding the whole picture locally, which means no
//! cross-instance query at resolve time and nothing to fall over.
//!
//! The mirror is a pull, not a push: an instance asks each sibling what it can
//! hear, and a sibling only ever reports what it heard *itself*. That asymmetry
//! is what stops two instances echoing each other's entries back and forth.

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use microdns_core::db::Db;
use microdns_core::types::RecordData;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::cache::Entry;

/// How long to wait on a sibling before giving up on this round.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The API port every instance in a fleet serves on. Used when a peer is given
/// as a bare address, and when peers are derived from DNS forwarders.
const DEFAULT_API_PORT: u16 = 8080;

/// One instance's answer to "what can you hear?".
#[derive(Debug, Deserialize)]
struct DiscoveredResponse {
    #[serde(default)]
    instance_id: String,
    #[serde(default)]
    entries: Vec<DiscoveredEntry>,
}

#[derive(Debug, Deserialize)]
struct DiscoveredEntry {
    name: String,
    data: RecordData,
    ttl: u32,
    expires_in: i64,
    from: String,
}

/// Pulls sibling discoveries over the REST API.
pub struct PeerSync {
    client: reqwest::Client,
    /// This instance's own id, so a peer list that happens to include us is
    /// skipped rather than mirrored back onto ourselves.
    instance_id: String,
}

impl PeerSync {
    pub fn new(instance_id: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            instance_id: instance_id.to_string(),
        }
    }

    /// Ask one sibling what it can hear. `None` means it could not be reached
    /// or is this instance itself — in both cases the caller keeps whatever it
    /// already had for that peer, so names age out on their own TTL rather than
    /// blinking out because one poll failed.
    pub async fn poll(&self, peer: &str) -> Option<Vec<Entry>> {
        let url = format!("http://{}/api/v1/mdns/discovered", with_port(peer));
        let response = match self.client.get(&url).send().await {
            Ok(response) => response,
            Err(e) => {
                debug!("mdns: peer {peer} unreachable ({e}); keeping its last known names");
                return None;
            }
        };
        if !response.status().is_success() {
            debug!("mdns: peer {peer} answered {}", response.status());
            return None;
        }

        let body: DiscoveredResponse = match response.json().await {
            Ok(body) => body,
            Err(e) => {
                warn!("mdns: peer {peer} sent something unreadable: {e}");
                return None;
            }
        };

        if !body.instance_id.is_empty() && body.instance_id == self.instance_id {
            debug!("mdns: skipping peer {peer} — it is this instance");
            return None;
        }

        let now = Utc::now();
        let entries = body
            .entries
            .into_iter()
            .filter_map(|e| {
                // A name already past its lifetime on the peer is not worth
                // republishing here.
                if e.expires_in <= 0 {
                    return None;
                }
                Some(Entry {
                    name: e.name,
                    data: e.data,
                    ttl: e.ttl,
                    first_seen: now,
                    last_seen: now,
                    // Expiry follows the peer's own clock reading, not ours, so
                    // a mirrored name never outlives the announcement behind it.
                    expires_at: now + ChronoDuration::seconds(e.expires_in),
                    from: e.from.parse().unwrap_or(std::net::IpAddr::V4(
                        std::net::Ipv4Addr::UNSPECIFIED,
                    )),
                    via: Some(peer.to_string()),
                    // Maintenance re-queries are the job of the instance that
                    // can actually hear the device.
                    refresh_sent: true,
                })
            })
            .collect();
        Some(entries)
    }
}

/// The peers to mirror: the configured list, or — when that is empty — the DNS
/// servers this instance already forwards to.
///
/// Deriving them is what keeps a growing fleet free of per-instance bookkeeping:
/// a new network arrives with a forwarder pointing at its DNS server, and that
/// server is exactly the instance that can hear that network's announcements.
pub fn resolve_peers(db: &Db, configured: &[String]) -> Vec<String> {
    if !configured.is_empty() {
        return configured.iter().map(|p| with_port(p)).collect();
    }

    let forwarders = match db.list_dns_forwarders() {
        Ok(forwarders) => forwarders,
        Err(e) => {
            warn!("mdns: could not read forwarders to find peers: {e}");
            return Vec::new();
        }
    };

    let mut peers: Vec<String> = forwarders
        .iter()
        .flat_map(|f| f.servers.iter())
        .filter_map(|server| {
            // Forwarders name a DNS port; the API lives on the same host.
            let host = server.rsplit_once(':').map_or(server.as_str(), |(h, _)| h);
            if host.is_empty() {
                return None;
            }
            Some(format!("{host}:{DEFAULT_API_PORT}"))
        })
        .collect();
    peers.sort();
    peers.dedup();
    peers
}

/// Add the API port to a bare address.
fn with_port(peer: &str) -> String {
    let peer = peer.trim();
    if peer.contains(':') {
        peer.to_string()
    } else {
        format!("{peer}:{DEFAULT_API_PORT}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use microdns_core::types::DnsForwarder;

    fn test_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.redb")).unwrap();
        (db, dir)
    }

    #[test]
    fn a_bare_address_gets_the_api_port() {
        assert_eq!(with_port("192.168.9.252"), "192.168.9.252:8080");
        assert_eq!(with_port("192.168.9.252:9090"), "192.168.9.252:9090");
        assert_eq!(with_port(" 192.168.9.252 "), "192.168.9.252:8080");
    }

    #[test]
    fn configured_peers_win_over_derived_ones() {
        let (db, _dir) = test_db();
        db.create_dns_forwarder(&DnsForwarder {
            zone: "g8.lo".into(),
            servers: vec!["192.168.8.252:53".into()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

        let peers = resolve_peers(&db, &["10.0.0.1".to_string()]);
        assert_eq!(peers, vec!["10.0.0.1:8080".to_string()]);
    }

    #[test]
    fn peers_are_derived_from_forwarders_once_deduplicated() {
        let (db, _dir) = test_db();
        for (zone, server) in [
            ("g8.lo", "192.168.8.252:53"),
            ("8.168.192.in-addr.arpa", "192.168.8.252:53"),
            ("g10.lo", "192.168.10.252:53"),
        ] {
            db.create_dns_forwarder(&DnsForwarder {
                zone: zone.into(),
                servers: vec![server.into()],
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .unwrap();
        }

        // Two zones point at the same instance; it is one peer, not two.
        assert_eq!(
            resolve_peers(&db, &[]),
            vec![
                "192.168.10.252:8080".to_string(),
                "192.168.8.252:8080".to_string()
            ]
        );
    }

    #[test]
    fn no_forwarders_means_no_peers_rather_than_a_guess() {
        let (db, _dir) = test_db();
        assert!(resolve_peers(&db, &[]).is_empty());
    }
}
