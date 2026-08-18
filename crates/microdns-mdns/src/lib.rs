//! mDNS ingest for MicroDNS.
//!
//! mDNS answers only travel one hop: queries go to `224.0.0.251` with an IP TTL
//! of 1, so every router on the way drops them by design. A device that
//! advertises `teslatracker-52c4.local` is therefore invisible to every client
//! outside its own segment, even though its address is perfectly routable.
//!
//! A MicroDNS instance already sits on the segment it serves, which is exactly
//! where those announcements are audible. This source listens to them, holds
//! what it hears for as long as the announced TTL says to, and publishes the
//! result as ordinary authoritative records in a zone of its own. Cross-subnet
//! clients then resolve the name over normal unicast DNS — no multicast
//! relaying between VLANs, and no hand-maintained A record per device.
//!
//! It runs in-process as one task of the microdns binary, the same way the
//! Kubernetes source does, and shares the process's redb database.
//!
//! ```no_run
//! # async fn run() -> anyhow::Result<()> {
//! use microdns_core::db::Db;
//! use microdns_mdns::{MdnsConfig, MdnsSource};
//!
//! let db = Db::open(std::path::Path::new("microdns.redb"))?;
//! let (_tx, shutdown) = tokio::sync::watch::channel(false);
//! let config = MdnsConfig { zone: "mdns.g9.lo".into(), ..Default::default() };
//! MdnsSource::new(db, config).run(shutdown).await?;
//! # Ok(()) }
//! ```

pub mod cache;
pub mod config;
pub mod parse;
pub mod publish;
pub mod socket;
pub mod translate;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use hickory_proto::op::Message;
use hickory_proto::serialize::binary::BinDecodable;
use microdns_core::db::Db;
use microdns_core::types::RecordType;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

pub use cache::{Entry, MdnsCache, Stats, SERVICE_ENUMERATION};
pub use config::MdnsConfig;
pub use publish::Applied;
pub use translate::DesiredRecord;

/// How long after startup the source publishes without withdrawing. The cache
/// starts empty on every restart, and an empty cache is not evidence that a
/// device has gone away — only that nothing has spoken yet.
const STARTUP_GRACE: Duration = Duration::from_secs(45);

/// Cap on maintenance re-queries per tick, so a large cache expiring at once
/// cannot turn into a burst of multicast traffic.
const MAX_REFRESH_PER_TICK: usize = 20;

/// Live view of the source, shared with the REST API.
#[derive(Clone)]
pub struct MdnsHandle {
    pub cache: Arc<Mutex<MdnsCache>>,
    pub zone: String,
    pub config: Arc<MdnsConfig>,
}

/// The mDNS source: listens, caches, and keeps its publish zone in sync.
pub struct MdnsSource {
    db: Db,
    config: MdnsConfig,
    cache: Arc<Mutex<MdnsCache>>,
}

impl MdnsSource {
    pub fn new(db: Db, config: MdnsConfig) -> Self {
        Self {
            db,
            config,
            cache: Arc::new(Mutex::new(MdnsCache::new())),
        }
    }

    /// A handle the API can read the live discovery table through.
    pub fn handle(&self) -> MdnsHandle {
        MdnsHandle {
            cache: self.cache.clone(),
            zone: self.config.zone.clone(),
            config: Arc::new(self.config.clone()),
        }
    }

    /// Listen and reconcile until `shutdown` flips to `true`.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        let v4 = socket::bind_v4(&self.config)?;
        let port = v4.local_addr()?.port();
        let v6 = if self.config.ipv6 {
            match socket::bind_v6(&self.config) {
                Ok(sock) => Some(sock),
                // A host without IPv6 configured is a normal deployment, not a
                // reason to give up the IPv4 half of the feature.
                Err(e) => {
                    warn!("mdns: IPv6 listener unavailable ({e}); continuing on IPv4 only");
                    None
                }
            }
        } else {
            None
        };

        let mut publisher = publish::Publisher::new(self.db.clone(), &self.config)?;
        info!(
            "mdns: listening on {}:{} — publishing discovered .local names into {}",
            self.config.bind,
            port,
            publisher.zone_name()
        );

        // Ask straight away rather than waiting a full interval: at startup the
        // cache is empty and everything interesting has already announced.
        self.send_queries(&v4, &v6, &[(SERVICE_ENUMERATION.to_string(), RecordType::PTR)])
            .await;

        let started = tokio::time::Instant::now();
        let mut reconcile_tick =
            tokio::time::interval(Duration::from_secs(self.config.debounce_secs.max(1)));
        reconcile_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut query_tick = tokio::time::interval(Duration::from_secs(
            // A zero interval means passive-only; the timer still has to have a
            // valid period, so it is parked far enough out never to matter.
            if self.config.query_interval_secs == 0 {
                86_400
            } else {
                self.config.query_interval_secs
            },
        ));
        query_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        query_tick.tick().await; // the first tick is immediate; we just queried

        let mut buf_v4 = vec![0u8; socket::MAX_PACKET];
        let mut buf_v6 = vec![0u8; socket::MAX_PACKET];
        let mut dirty = false;

        loop {
            tokio::select! {
                result = v4.recv_from(&mut buf_v4) => {
                    match result {
                        Ok((len, from)) => {
                            dirty |= self.absorb(&buf_v4[..len], from.ip());
                        }
                        Err(e) => warn!("mdns: IPv4 receive failed: {e}"),
                    }
                }
                result = recv_optional(v6.as_ref(), &mut buf_v6) => {
                    match result {
                        Ok((len, from)) => {
                            dirty |= self.absorb(&buf_v6[..len], from.ip());
                        }
                        Err(e) => warn!("mdns: IPv6 receive failed: {e}"),
                    }
                }
                _ = reconcile_tick.tick() => {
                    let expired = self.cache.lock().unwrap().expire(Utc::now());
                    if expired > 0 {
                        debug!("mdns: {expired} cached record(s) expired");
                    }
                    if dirty || expired > 0 {
                        let prune = started.elapsed() >= STARTUP_GRACE;
                        let desired = {
                            let cache = self.cache.lock().unwrap();
                            translate::desired(cache.entries().cloned(), &self.config)
                        };
                        match publisher.apply(&desired, prune) {
                            Ok(_) => dirty = false,
                            // Keep the dirty flag set so a transient database
                            // error is retried on the next tick.
                            Err(e) => warn!("mdns: publishing to {} failed: {e}", publisher.zone_name()),
                        }
                    }

                    let due = self.cache.lock().unwrap().take_refresh_due(Utc::now());
                    if !due.is_empty() && self.config.query_interval_secs > 0 {
                        let batch: Vec<_> = due.into_iter().take(MAX_REFRESH_PER_TICK).collect();
                        self.send_queries(&v4, &v6, &batch).await;
                    }
                }
                _ = query_tick.tick(), if self.config.query_interval_secs > 0 => {
                    let mut questions = vec![(SERVICE_ENUMERATION.to_string(), RecordType::PTR)];
                    for service in self.cache.lock().unwrap().service_types() {
                        questions.push((service, RecordType::PTR));
                    }
                    self.send_queries(&v4, &v6, &questions).await;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("mdns: shutdown requested");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Parse one packet into the cache. Returns whether the published set needs
    /// rewriting.
    fn absorb(&self, packet: &[u8], from: std::net::IpAddr) -> bool {
        let message = match Message::from_bytes(packet) {
            Ok(message) => message,
            // Malformed multicast is normal on a busy LAN — some responders
            // emit records this parser does not know. Never let it be fatal.
            Err(e) => {
                debug!("mdns: unparseable packet from {from}: {e}");
                return false;
            }
        };

        let announcements = parse::announcements(&message, from);
        let now = Utc::now();
        let mut cache = self.cache.lock().unwrap();
        cache.stats.packets += 1;
        cache.last_packet_at = Some(now);

        let mut changed = false;
        for announcement in announcements {
            // A goodbye keeps its zero TTL — clamping it would resurrect the
            // record for ttl_min seconds.
            let ttl = if announcement.ttl == 0 {
                0
            } else {
                self.config.clamp_ttl(announcement.ttl)
            };
            let outcome = cache.learn(&announcement.name, announcement.data, ttl, from, now);
            changed |= outcome.changes_zone();
        }
        changed
    }

    /// Multicast a batch of questions on every listener we have.
    async fn send_queries(
        &self,
        v4: &UdpSocket,
        v6: &Option<UdpSocket>,
        questions: &[(String, RecordType)],
    ) {
        let Some(wire) = socket::query(questions) else {
            return;
        };
        let port = v4.local_addr().map(|a| a.port()).unwrap_or(config::MDNS_PORT);

        if let Err(e) = v4.send_to(&wire, socket::group_v4(port)).await {
            debug!("mdns: IPv4 query send failed: {e}");
        }
        if let Some(sock) = v6 {
            if let Err(e) = sock.send_to(&wire, socket::group_v6(port)).await {
                debug!("mdns: IPv6 query send failed: {e}");
            }
        }
        let mut cache = self.cache.lock().unwrap();
        cache.stats.queries_sent += questions.len() as u64;
    }
}

/// Receive from the IPv6 socket if there is one; otherwise never resolve, so
/// `select!` simply ignores that branch.
async fn recv_optional(
    socket: Option<&UdpSocket>,
    buf: &mut [u8],
) -> std::io::Result<(usize, std::net::SocketAddr)> {
    match socket {
        Some(socket) => socket.recv_from(buf).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{Header, MessageType, OpCode};
    use hickory_proto::rr::rdata::A;
    use hickory_proto::rr::{Name, RData, Record as ProtoRecord};
    use hickory_proto::serialize::binary::BinEncodable;
    use microdns_core::types::{RecordData, RecordSource};
    use std::str::FromStr;

    fn test_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.redb")).unwrap();
        (db, dir)
    }

    fn announcement_packet(name: &str, ip: [u8; 4], ttl: u32) -> Vec<u8> {
        let mut header = Header::new();
        header.set_message_type(MessageType::Response);
        header.set_op_code(OpCode::Query);
        header.set_authoritative(true);

        let mut record = ProtoRecord::new();
        record.set_name(Name::from_str(name).unwrap());
        record.set_ttl(ttl);
        record.set_record_type(hickory_proto::rr::RecordType::A);
        record.set_data(Some(RData::A(A::new(ip[0], ip[1], ip[2], ip[3]))));

        let mut message = Message::new();
        message.set_header(header);
        message.add_answer(record);
        message.to_bytes().unwrap()
    }

    #[test]
    fn an_announcement_lands_in_the_cache_and_then_in_the_zone() {
        let (db, _dir) = test_db();
        let config = MdnsConfig {
            zone: "mdns.g9.lo".into(),
            ..Default::default()
        };
        let source = MdnsSource::new(db.clone(), config.clone());

        let packet = announcement_packet("teslatracker-52c4.local.", [192, 168, 9, 134], 120);
        assert!(source.absorb(&packet, "192.168.9.134".parse().unwrap()));

        let desired = {
            let cache = source.cache.lock().unwrap();
            assert_eq!(cache.len(), 1);
            translate::desired(cache.entries().cloned(), &config)
        };

        let mut publisher = publish::Publisher::new(db.clone(), &config).unwrap();
        publisher.apply(&desired, true).unwrap();

        let records = db
            .query_fqdn("teslatracker-52c4.mdns.g9.lo", RecordType::A)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].data,
            RecordData::A("192.168.9.134".parse().unwrap())
        );
        assert_eq!(records[0].source, RecordSource::Mdns);
        assert_eq!(records[0].ttl, 120);
    }

    #[test]
    fn a_goodbye_withdraws_the_name_again() {
        let (db, _dir) = test_db();
        let config = MdnsConfig {
            zone: "mdns.g9.lo".into(),
            ..Default::default()
        };
        let source = MdnsSource::new(db.clone(), config.clone());
        let mut publisher = publish::Publisher::new(db.clone(), &config).unwrap();

        let from = "192.168.9.134".parse().unwrap();
        source.absorb(
            &announcement_packet("teslatracker-52c4.local.", [192, 168, 9, 134], 120),
            from,
        );
        let desired = translate::desired(
            source.cache.lock().unwrap().entries().cloned(),
            &config,
        );
        publisher.apply(&desired, true).unwrap();

        // TTL 0 is the device saying goodbye as it leaves.
        assert!(source.absorb(
            &announcement_packet("teslatracker-52c4.local.", [192, 168, 9, 134], 0),
            from
        ));
        let desired = translate::desired(
            source.cache.lock().unwrap().entries().cloned(),
            &config,
        );
        assert!(desired.is_empty());
        publisher.apply(&desired, true).unwrap();

        assert!(db
            .query_fqdn("teslatracker-52c4.mdns.g9.lo", RecordType::A)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_garbled_packet_is_ignored_rather_than_fatal() {
        let (db, _dir) = test_db();
        let source = MdnsSource::new(db, MdnsConfig::default());
        assert!(!source.absorb(b"not a dns packet", "192.168.9.1".parse().unwrap()));
        assert!(source.cache.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_source_stops_when_shutdown_is_signalled() {
        let (db, _dir) = test_db();
        let config = MdnsConfig {
            zone: "mdns.test.lo".into(),
            // Port 0 keeps the test off the real mDNS port.
            port: 0,
            ..Default::default()
        };
        let (tx, rx) = watch::channel(false);
        let source = MdnsSource::new(db, config);
        let task = tokio::spawn(source.run(rx));

        tx.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("source should stop promptly");
        result.unwrap().unwrap();
    }
}
