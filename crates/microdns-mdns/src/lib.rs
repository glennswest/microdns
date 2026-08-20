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
//! Its configuration lives in the database (`runtime_config`, section `mdns`)
//! rather than in the process, so the source is a supervisor: it follows
//! whatever the stored config currently says, starting, stopping and re-homing
//! its zone without a restart. Deployments whose TOML is generated for them —
//! mkube renders one per network — can therefore turn discovery on through the
//! API, and have it stay on.
//!
//! ```no_run
//! # async fn run() -> anyhow::Result<()> {
//! use microdns_core::db::Db;
//! use microdns_mdns::MdnsSource;
//!
//! let db = Db::open(std::path::Path::new("microdns.redb"))?;
//! let (_tx, shutdown) = tokio::sync::watch::channel(false);
//! MdnsSource::new(db).run(shutdown).await?;
//! # Ok(()) }
//! ```

pub mod cache;
pub mod config;
pub mod parse;
pub mod publish;
pub mod sink;
pub mod socket;
pub mod translate;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use hickory_proto::op::Message;
use hickory_proto::serialize::binary::BinDecodable;
use microdns_core::config::MdnsSourceConfig;
use microdns_core::db::Db;
use microdns_core::types::RecordType;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::{debug, info, warn};

pub use cache::{Entry, MdnsCache, Stats, SERVICE_ENUMERATION};
pub use config::MdnsConfig;
pub use publish::Applied;
pub use translate::DesiredRecord;

/// Database section the stored configuration lives under.
pub const CONFIG_SECTION: &str = "mdns";

/// How long after starting to listen the source publishes without withdrawing.
/// The cache starts empty every time, and an empty cache is not evidence that a
/// device has gone away — only that nothing has spoken yet.
const STARTUP_GRACE: Duration = Duration::from_secs(45);

/// Cap on maintenance re-queries per tick, so a large cache expiring at once
/// cannot turn into a burst of multicast traffic.
const MAX_REFRESH_PER_TICK: usize = 20;

/// How often the stored configuration is re-read. Cheap: redb is memory-mapped,
/// so this is a read of one small value.
const CONFIG_POLL: Duration = Duration::from_secs(10);

/// Live view of the source, shared with the REST API.
#[derive(Clone)]
pub struct MdnsHandle {
    pub cache: Arc<Mutex<MdnsCache>>,
    /// Config the source is running under right now, or `None` while disabled.
    pub current: Arc<Mutex<Option<MdnsConfig>>>,
}

impl MdnsHandle {
    /// Config to interpret the cache with — the live one while running, and the
    /// defaults otherwise, so a disabled source still renders a sane table.
    pub fn config(&self) -> MdnsConfig {
        self.current.lock().unwrap().clone().unwrap_or_default()
    }

    /// Zone being published to, or `None` while the source is disabled.
    pub fn zone(&self) -> Option<String> {
        self.current.lock().unwrap().as_ref().map(|c| c.zone.clone())
    }
}

/// Why a listening session ended.
enum SessionEnd {
    ConfigChanged,
    Shutdown,
}

/// The mDNS source: listens, caches, and keeps its publish zone in sync.
pub struct MdnsSource {
    db: Db,
    instance_id: String,
    cache: Arc<Mutex<MdnsCache>>,
    current: Arc<Mutex<Option<MdnsConfig>>>,
}

impl MdnsSource {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            instance_id: String::new(),
            cache: Arc::new(Mutex::new(MdnsCache::new())),
            current: Arc::new(Mutex::new(None)),
        }
    }

    /// This instance's id, so a derived peer list that includes this instance
    /// is skipped rather than mirrored onto itself.
    pub fn with_instance_id(mut self, id: &str) -> Self {
        self.instance_id = id.to_string();
        self
    }

    /// A handle the API can read the live discovery table through.
    pub fn handle(&self) -> MdnsHandle {
        MdnsHandle {
            cache: self.cache.clone(),
            current: self.current.clone(),
        }
    }

    /// Copy a bootstrap `[mdns]` block into the database if nothing is stored
    /// yet, mirroring how pools and forwarders are seeded from TOML. Returns
    /// whether anything was written.
    pub fn bootstrap(db: &Db, config: &MdnsSourceConfig) -> anyhow::Result<bool> {
        if db
            .get_runtime_section::<MdnsSourceConfig>(CONFIG_SECTION)?
            .is_some()
        {
            return Ok(false);
        }
        db.set_runtime_section(CONFIG_SECTION, config)?;
        Ok(true)
    }

    /// Follow the stored configuration until shutdown: listen while it says
    /// enabled, idle while it does not, and re-home the zone when it changes.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        loop {
            match self.stored_config() {
                Some(config) if config.enabled => {
                    *self.current.lock().unwrap() = Some(config.clone());
                    let end = self.session(&config, &mut shutdown).await;
                    *self.current.lock().unwrap() = None;
                    self.cache.lock().unwrap().clear();

                    match end {
                        Ok(SessionEnd::Shutdown) => return Ok(()),
                        Ok(SessionEnd::ConfigChanged) => {
                            // Whatever was published came from this instance's
                            // authority. If the new config does not cover it,
                            // it must not be left behind claiming to be current.
                            self.withdraw(&config).await;
                            info!("mdns: configuration changed; reloading");
                        }
                        Err(e) => {
                            // A listener that cannot bind (no multicast on this
                            // interface, port already taken) must not take DNS
                            // down and must not spin: report it, then wait for
                            // the configuration to change.
                            warn!("mdns: listener stopped: {e}");
                            if self.wait_for_change(&mut shutdown, Some(&config)).await {
                                return Ok(());
                            }
                        }
                    }
                }
                other => {
                    if let Some(config) = other {
                        // Turned off explicitly: take the discovered names with
                        // it rather than leaving them behind.
                        self.withdraw(&config).await;
                    }
                    if self.wait_for_change(&mut shutdown, None).await {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Read the stored configuration, or `None` when nothing is stored.
    fn stored_config(&self) -> Option<MdnsConfig> {
        match self
            .db
            .get_runtime_section::<MdnsSourceConfig>(CONFIG_SECTION)
        {
            Ok(Some(stored)) => Some(MdnsConfig::from(&stored)),
            Ok(None) => None,
            Err(e) => {
                warn!("mdns: could not read stored config: {e}");
                None
            }
        }
    }

    /// Wait until the stored config differs from `previous`, or until shutdown.
    /// Returns true when shutdown won.
    async fn wait_for_change(
        &self,
        shutdown: &mut watch::Receiver<bool>,
        previous: Option<&MdnsConfig>,
    ) -> bool {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(CONFIG_POLL) => {
                    let now = self.stored_config();
                    let changed = match (previous, &now) {
                        (Some(before), Some(after)) => before != after,
                        // Nothing was stored before: only an enabled config is
                        // worth waking for.
                        (None, Some(after)) => after.enabled,
                        (Some(_), None) => true,
                        (None, None) => false,
                    };
                    if changed {
                        return false;
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        return true;
                    }
                }
            }
        }
    }

    /// Remove everything this instance put in the zone, wherever it is held.
    async fn withdraw(&self, config: &MdnsConfig) {
        let sink = match sink::ZoneSink::new(self.db.clone(), config, &self.instance_id) {
            Ok(sink) => sink,
            Err(e) => {
                warn!("mdns: could not reach {} to withdraw: {e}", config.zone);
                return;
            }
        };
        match sink.withdraw_all().await {
            Ok(0) => {}
            Ok(n) => info!("mdns: withdrew {n} discovered name(s) from {}", config.zone),
            Err(e) => warn!("mdns: could not withdraw names from {}: {e}", config.zone),
        }
        // Stop pointing this instance's clients at a zone it no longer feeds.
        sink::remove_forwarder(&self.db, config);
    }

    /// One listening session under a fixed configuration.
    async fn session(
        &self,
        config: &MdnsConfig,
        shutdown: &mut watch::Receiver<bool>,
    ) -> anyhow::Result<SessionEnd> {
        let v4 = socket::bind_v4(config)?;
        let port = v4.local_addr()?.port();
        let v6 = if config.ipv6 {
            match socket::bind_v6(config) {
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

        let mut sink = sink::ZoneSink::new(self.db.clone(), config, &self.instance_id)?;
        // A reporting instance answers its own clients for the shared zone by
        // pointing them at whoever holds it.
        sink::ensure_forwarder(&self.db, config);
        info!(
            "mdns: listening on {}:{} — registering discovered .local names in {}",
            config.bind,
            port,
            sink.describe()
        );

        // Ask straight away rather than waiting a full interval: at startup the
        // cache is empty and everything interesting has already announced.
        self.send_queries(
            config,
            &v4,
            &v6,
            &[(SERVICE_ENUMERATION.to_string(), RecordType::PTR)],
        )
        .await;

        let started = tokio::time::Instant::now();
        let mut reconcile_tick =
            tokio::time::interval(Duration::from_secs(config.debounce_secs.max(1)));
        reconcile_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut query_tick = tokio::time::interval(Duration::from_secs(
            // A zero interval means passive-only; the timer still has to have a
            // valid period, so it is parked far enough out never to matter.
            if config.query_interval_secs == 0 {
                86_400
            } else {
                config.query_interval_secs
            },
        ));
        query_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        query_tick.tick().await; // the first tick is immediate; we just queried

        let mut config_tick = tokio::time::interval(CONFIG_POLL);
        config_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        config_tick.tick().await;

        let mut buf_v4 = vec![0u8; socket::MAX_PACKET];
        let mut buf_v6 = vec![0u8; socket::MAX_PACKET];
        let mut dirty = false;

        loop {
            tokio::select! {
                result = v4.recv_from(&mut buf_v4) => {
                    match result {
                        Ok((len, from)) => {
                            dirty |= self.absorb(config, &buf_v4[..len], from.ip());
                        }
                        Err(e) => warn!("mdns: IPv4 receive failed: {e}"),
                    }
                }
                result = recv_optional(v6.as_ref(), &mut buf_v6) => {
                    match result {
                        Ok((len, from)) => {
                            dirty |= self.absorb(config, &buf_v6[..len], from.ip());
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
                            translate::desired(cache.entries().cloned(), config)
                        };
                        match sink.apply(&desired, prune).await {
                            Ok(_) => dirty = false,
                            // Keep the dirty flag set so a transient failure —
                            // a database error, or the holder being restarted —
                            // is retried on the next tick.
                            Err(e) => warn!("mdns: registering in {} failed: {e}", config.zone),
                        }
                    }

                    let due = self.cache.lock().unwrap().take_refresh_due(Utc::now());
                    if !due.is_empty() && config.query_interval_secs > 0 {
                        let batch: Vec<_> = due.into_iter().take(MAX_REFRESH_PER_TICK).collect();
                        self.send_queries(config, &v4, &v6, &batch).await;
                    }
                }
                _ = query_tick.tick(), if config.query_interval_secs > 0 => {
                    let mut questions = vec![(SERVICE_ENUMERATION.to_string(), RecordType::PTR)];
                    for service in self.cache.lock().unwrap().service_types() {
                        questions.push((service, RecordType::PTR));
                    }
                    self.send_queries(config, &v4, &v6, &questions).await;
                }
                _ = config_tick.tick() => {
                    if self.stored_config().as_ref() != Some(config) {
                        return Ok(SessionEnd::ConfigChanged);
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("mdns: shutdown requested");
                        return Ok(SessionEnd::Shutdown);
                    }
                }
            }
        }
    }

    /// Parse one packet into the cache. Returns whether the published set needs
    /// rewriting.
    fn absorb(&self, config: &MdnsConfig, packet: &[u8], from: std::net::IpAddr) -> bool {
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
                config.clamp_ttl(announcement.ttl)
            };
            let outcome = cache.learn(&announcement.name, announcement.data, ttl, from, now);
            changed |= outcome.changes_zone();
        }
        changed
    }

    /// Multicast a batch of questions on every listener we have.
    async fn send_queries(
        &self,
        config: &MdnsConfig,
        v4: &UdpSocket,
        v6: &Option<UdpSocket>,
        questions: &[(String, RecordType)],
    ) {
        let Some(wire) = socket::query(questions) else {
            return;
        };
        let port = v4.local_addr().map(|a| a.port()).unwrap_or(config.port);

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

    fn stored(zone: &str, enabled: bool) -> MdnsSourceConfig {
        MdnsSourceConfig {
            enabled,
            zone: zone.into(),
            ttl_min: 60,
            ttl_max: 1200,
            services: true,
            allow: vec![],
            deny: vec![],
            query_interval_secs: 300,
            ipv6: false,
            bind: "0.0.0.0".into(),
            interfaces: vec![],
            debounce_secs: 5,
            holder: String::new(),
        }
    }

    fn config(zone: &str) -> MdnsConfig {
        MdnsConfig {
            enabled: true,
            zone: zone.into(),
            ..Default::default()
        }
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
        let config = config("mdns.g9.lo");
        let source = MdnsSource::new(db.clone());

        let packet = announcement_packet("teslatracker-52c4.local.", [192, 168, 9, 134], 120);
        assert!(source.absorb(&config, &packet, "192.168.9.134".parse().unwrap()));

        let desired = {
            let cache = source.cache.lock().unwrap();
            assert_eq!(cache.len(), 1);
            translate::desired(cache.entries().cloned(), &config)
        };

        let mut publisher = publish::Publisher::new(db.clone(), &config, "test").unwrap();
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
        let config = config("mdns.g9.lo");
        let source = MdnsSource::new(db.clone());
        let mut publisher = publish::Publisher::new(db.clone(), &config, "test").unwrap();

        let from = "192.168.9.134".parse().unwrap();
        source.absorb(
            &config,
            &announcement_packet("teslatracker-52c4.local.", [192, 168, 9, 134], 120),
            from,
        );
        let desired = translate::desired(source.cache.lock().unwrap().entries().cloned(), &config);
        publisher.apply(&desired, true).unwrap();

        // TTL 0 is the device saying goodbye as it leaves.
        assert!(source.absorb(
            &config,
            &announcement_packet("teslatracker-52c4.local.", [192, 168, 9, 134], 0),
            from
        ));
        let desired = translate::desired(source.cache.lock().unwrap().entries().cloned(), &config);
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
        let source = MdnsSource::new(db);
        assert!(!source.absorb(
            &MdnsConfig::default(),
            b"not a dns packet",
            "192.168.9.1".parse().unwrap()
        ));
        assert!(source.cache.lock().unwrap().is_empty());
    }

    #[test]
    fn bootstrap_seeds_the_database_once_and_then_defers_to_it() {
        let (db, _dir) = test_db();

        assert!(MdnsSource::bootstrap(&db, &stored("mdns.g9.lo", true)).unwrap());
        // An operator changes it through the API...
        db.set_runtime_section(CONFIG_SECTION, &stored("discovered.g9.lo", true))
            .unwrap();
        // ...and a restart carrying the old TOML must not undo that.
        assert!(!MdnsSource::bootstrap(&db, &stored("mdns.g9.lo", true)).unwrap());

        let source = MdnsSource::new(db);
        assert_eq!(source.stored_config().unwrap().zone, "discovered.g9.lo");
    }

    #[test]
    fn no_stored_config_means_no_source() {
        let (db, _dir) = test_db();
        assert!(MdnsSource::new(db).stored_config().is_none());
    }

    #[tokio::test]
    async fn a_disabled_source_idles_and_still_shuts_down_promptly() {
        let (db, _dir) = test_db();
        db.set_runtime_section(CONFIG_SECTION, &stored("mdns.test.lo", false))
            .unwrap();

        let (tx, rx) = watch::channel(false);
        let source = MdnsSource::new(db);
        let handle = source.handle();
        let task = tokio::spawn(source.run(rx));

        // Disabled: nothing is listening, and the API sees no zone.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(handle.zone().is_none());

        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .expect("a disabled source should still stop promptly")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn turning_the_source_off_withdraws_only_what_it_published() {
        let (db, _dir) = test_db();
        let config = config("mdns.test.lo");

        // One discovered record, and one curated record that must survive.
        let mut publisher = publish::Publisher::new(db.clone(), &config, "test").unwrap();
        publisher
            .apply(
                &[DesiredRecord {
                    name: "tracker".into(),
                    ttl: 120,
                    data: RecordData::A("192.168.9.134".parse().unwrap()),
                }],
                true,
            )
            .unwrap();
        let zone = db.get_zone_by_name("mdns.test.lo").unwrap().unwrap();
        db.create_record(&microdns_core::types::Record {
            id: uuid::Uuid::new_v4(),
            zone_id: zone.id,
            name: "curated".into(),
            ttl: 300,
            data: RecordData::A("10.0.0.1".parse().unwrap()),
            enabled: true,
            health_check: None,
            source: RecordSource::Manual,
            origin: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

        // Withdrawing under a different instance id must take nothing: that
        // record belongs to whoever registered it.
        MdnsSource::new(db.clone())
            .with_instance_id("someone-else")
            .withdraw(&config)
            .await;
        assert_eq!(db.list_records(&zone.id).unwrap().len(), 2);

        MdnsSource::new(db.clone())
            .with_instance_id("test")
            .withdraw(&config)
            .await;

        let records = db.list_records(&zone.id).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "curated");
    }
}
