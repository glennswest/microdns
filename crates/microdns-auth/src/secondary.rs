//! Secondary (slave) zones: mirror a zone from a primary over AXFR.
//!
//! This is the receiving half of DNS NOTIFY. A primary announcing a change is
//! only useful if something acts on it, and a secondary that only polls is only
//! as fresh as its refresh timer — an hour on these zones, which is exactly the
//! window in which a fallback server would be serving visibly wrong data.
//!
//! So the agent watches two clocks. A NOTIFY from the zone's primary triggers an
//! immediate check; the refresh timer catches everything a lost NOTIFY missed,
//! since NOTIFY is a hint and not a delivery guarantee (RFC 1996 §4).
//!
//! Either way the check is the same and it is cheap: ask the primary for the
//! zone's SOA, and only transfer when its serial differs from the local copy.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::{Name, RData, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use microdns_core::db::Db;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::transfer::ZoneTransfer;

/// How long to wait for a primary to answer an SOA probe.
const SOA_TIMEOUT: Duration = Duration::from_secs(3);

/// How often the agent wakes to see which zones are due. Individual zones keep
/// their own refresh interval; this is just the resolution of the check.
const TICK: Duration = Duration::from_secs(15);

/// After a NOTIFY, wait this long before checking. A primary that changes ten
/// records sends ten NOTIFYs, and transferring the zone ten times would be a
/// self-inflicted denial of service on both ends.
const NOTIFY_SETTLE: Duration = Duration::from_secs(2);

/// One zone this instance mirrors.
#[derive(Debug, Clone)]
pub struct SecondaryZone {
    /// Zone name, lowercased, no trailing dot.
    pub zone: String,
    pub primary: SocketAddr,
    /// Fallback poll interval for when no NOTIFY arrives.
    pub refresh: Duration,
}

impl SecondaryZone {
    /// Parse one configured entry. `primary` accepts `ip` or `ip:port`.
    pub fn parse(zone: &str, primary: &str, refresh_secs: u64) -> Option<Self> {
        let addr = parse_addr(primary)?;
        let zone = zone.trim().trim_end_matches('.').to_lowercase();
        if zone.is_empty() {
            return None;
        }
        Some(Self {
            zone,
            primary: addr,
            refresh: Duration::from_secs(refresh_secs.max(30)),
        })
    }
}

/// Accepts inbound NOTIFY on behalf of the agent.
///
/// Held by the DNS server, which knows nothing about transfers — it only needs
/// to decide whether this sender is allowed to speak for this zone, and to pass
/// the name along.
#[derive(Clone)]
pub struct NotifyAcceptor {
    /// Zone name (lowercase, no trailing dot) → the address of its primary.
    primaries: Arc<HashMap<String, IpAddr>>,
    tx: mpsc::UnboundedSender<String>,
}

impl NotifyAcceptor {
    fn new(zones: &[SecondaryZone], tx: mpsc::UnboundedSender<String>) -> Self {
        let primaries = zones
            .iter()
            .map(|z| (z.zone.clone(), z.primary.ip()))
            .collect();
        Self {
            primaries: Arc::new(primaries),
            tx,
        }
    }

    /// Whether `peer` may announce a change to `zone`, and if so, queue it.
    ///
    /// Only the configured primary is believed. A NOTIFY is an instruction to
    /// go and transfer a zone, so accepting one from anywhere would let any
    /// host on the network aim this instance's transfers wherever it liked.
    pub fn accept(&self, peer: IpAddr, zone: &str) -> bool {
        let zone = zone.trim_end_matches('.').to_lowercase();
        let Some(primary) = self.primaries.get(&zone) else {
            return false;
        };
        if *primary != normalize(peer) {
            return false;
        }
        let _ = self.tx.send(zone);
        true
    }
}

/// Mirrors the configured zones, driven by NOTIFY and by each zone's timer.
pub struct SecondaryAgent {
    db: Db,
    zones: Vec<SecondaryZone>,
    notify_rx: mpsc::UnboundedReceiver<String>,
}

impl SecondaryAgent {
    /// Build the agent and the acceptor the DNS server hands NOTIFYs to.
    pub fn new(db: Db, zones: Vec<SecondaryZone>) -> (Self, NotifyAcceptor) {
        let (tx, notify_rx) = mpsc::unbounded_channel();
        let acceptor = NotifyAcceptor::new(&zones, tx);
        (
            Self {
                db,
                zones,
                notify_rx,
            },
            acceptor,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }

    /// Check every zone once, then keep them current until shutdown.
    pub async fn run(mut self, mut shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        info!(
            "secondary: mirroring {} zone(s): {}",
            self.zones.len(),
            self.zones
                .iter()
                .map(|z| format!("{} from {}", z.zone, z.primary))
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Next scheduled check per zone, by index. Everything is due at once at
        // startup so a secondary that was down comes back current immediately.
        let mut due: Vec<tokio::time::Instant> = vec![tokio::time::Instant::now(); self.zones.len()];
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                Some(zone) = self.notify_rx.recv() => {
                    // Collapse a burst: drain anything already queued, then let
                    // the settle window catch the rest before transferring.
                    let mut names = vec![zone];
                    while let Ok(more) = self.notify_rx.try_recv() {
                        names.push(more);
                    }
                    tokio::time::sleep(NOTIFY_SETTLE).await;
                    while let Ok(more) = self.notify_rx.try_recv() {
                        names.push(more);
                    }
                    names.sort();
                    names.dedup();

                    for name in names {
                        let Some(index) = self.zones.iter().position(|z| z.zone == name) else {
                            continue;
                        };
                        info!("secondary: NOTIFY for {name}; checking primary");
                        self.check(index).await;
                        due[index] = tokio::time::Instant::now() + self.zones[index].refresh;
                    }
                }
                _ = tick.tick() => {
                    let now = tokio::time::Instant::now();
                    for index in 0..self.zones.len() {
                        if due[index] > now {
                            continue;
                        }
                        self.check(index).await;
                        due[index] = tokio::time::Instant::now() + self.zones[index].refresh;
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("secondary: shutdown requested");
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// Compare serials with the primary and transfer when they differ.
    async fn check(&self, index: usize) {
        let zone = &self.zones[index];

        let remote = match remote_serial(&zone.zone, zone.primary).await {
            Ok(serial) => serial,
            // A primary that is down is exactly when the local copy matters, so
            // this is a warning and not an error: we keep serving what we have.
            Err(e) => {
                warn!(
                    "secondary: {} — could not read SOA from {}: {e}; serving the existing copy",
                    zone.zone, zone.primary
                );
                return;
            }
        };

        let local = self
            .db
            .get_zone_by_name(&zone.zone)
            .ok()
            .flatten()
            .map(|z| z.soa.serial);

        match local {
            Some(local) if local == remote => {
                debug!("secondary: {} is current at serial {local}", zone.zone);
                return;
            }
            Some(local) if !is_newer(remote, local) => {
                // The primary's serial went backwards — a restore, or a zone
                // rebuilt from scratch. Mirror it anyway: matching the primary
                // is the whole job, and refusing would strand the copy.
                warn!(
                    "secondary: {} — primary serial {remote} is older than local {local}; \
                     mirroring it anyway",
                    zone.zone
                );
            }
            Some(local) => debug!(
                "secondary: {} — primary at {remote}, local at {local}; transferring",
                zone.zone
            ),
            None => info!("secondary: {} not held locally yet; transferring", zone.zone),
        }

        let transfer = ZoneTransfer::new(self.db.clone());
        match transfer.axfr_pull(&zone.zone, zone.primary).await {
            Ok(result) => info!(
                "secondary: {} transferred from {} — {} records at serial {}",
                zone.zone, zone.primary, result.records_imported, result.serial
            ),
            Err(e) => warn!(
                "secondary: {} — transfer from {} failed: {e}; serving the existing copy",
                zone.zone, zone.primary
            ),
        }
    }
}

/// Ask a primary for a zone's SOA serial over UDP.
pub async fn remote_serial(zone: &str, primary: SocketAddr) -> anyhow::Result<u32> {
    let name = Name::from_utf8(format!("{}.", zone.trim_end_matches('.')))?;
    let mut query = Query::new();
    query.set_name(name);
    query.set_query_type(RecordType::SOA);

    let mut message = Message::new();
    message.set_id(query_id());
    message.set_message_type(MessageType::Query);
    message.set_op_code(OpCode::Query);
    message.set_recursion_desired(false);
    message.add_query(query);
    let wire = message.to_bytes()?;

    let bind: SocketAddr = if primary.is_ipv4() {
        "0.0.0.0:0".parse()?
    } else {
        "[::]:0".parse()?
    };
    let socket = UdpSocket::bind(bind).await?;
    socket.send_to(&wire, primary).await?;

    let mut buf = vec![0u8; 4096];
    let len = match tokio::time::timeout(SOA_TIMEOUT, socket.recv(&mut buf)).await {
        Ok(Ok(len)) => len,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => anyhow::bail!("no SOA response within {SOA_TIMEOUT:?}"),
    };

    let response = Message::from_bytes(&buf[..len])?;
    if response.response_code() != hickory_proto::op::ResponseCode::NoError {
        anyhow::bail!("primary answered {}", response.response_code());
    }

    // Authoritative answers put the SOA in the answer section; a server that
    // treats the name as a referral puts it in the authority section instead.
    response
        .answers()
        .iter()
        .chain(response.name_servers())
        .find_map(|record| match record.data() {
            Some(RData::SOA(soa)) => Some(soa.serial()),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("no SOA record in the response"))
}

/// Serial comparison per RFC 1982 §3.2: serials wrap, so "bigger number" is not
/// the same as "newer".
fn is_newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < 1 << 31
}

/// Parse `192.168.1.252` or `192.168.1.252:53`, defaulting to port 53.
fn parse_addr(target: &str) -> Option<SocketAddr> {
    let t = target.trim();
    if let Ok(addr) = t.parse::<SocketAddr>() {
        return Some(addr);
    }
    t.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 53))
}

/// Unwrap an IPv4-mapped IPv6 peer (`::ffff:192.168.1.1`) so a dual-stack
/// listener compares equal to an IPv4 primary.
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        other => other,
    }
}

fn query_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static COUNTER: AtomicU16 = AtomicU16::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u16)
        .unwrap_or(0);
    COUNTER.fetch_add(1, Ordering::Relaxed) ^ nanos
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zones() -> Vec<SecondaryZone> {
        vec![SecondaryZone::parse("gw.lo", "192.168.1.252", 900).unwrap()]
    }

    #[test]
    fn config_entries_parse_with_and_without_a_port() {
        let z = SecondaryZone::parse("gw.lo.", "192.168.1.252", 900).unwrap();
        assert_eq!(z.zone, "gw.lo");
        assert_eq!(z.primary, "192.168.1.252:53".parse().unwrap());

        let z = SecondaryZone::parse("GW.lo", "192.168.1.252:5353", 900).unwrap();
        assert_eq!(z.primary, "192.168.1.252:5353".parse().unwrap());

        assert!(SecondaryZone::parse("gw.lo", "not-an-address", 900).is_none());
        assert!(SecondaryZone::parse("", "192.168.1.252", 900).is_none());
    }

    #[test]
    fn a_refresh_far_below_the_tick_is_raised_to_something_sane() {
        let z = SecondaryZone::parse("gw.lo", "192.168.1.252", 1).unwrap();
        assert_eq!(z.refresh, Duration::from_secs(30));
    }

    #[test]
    fn only_the_configured_primary_may_announce_a_zone() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let acceptor = NotifyAcceptor::new(&zones(), tx);

        assert!(acceptor.accept("192.168.1.252".parse().unwrap(), "gw.lo."));
        assert_eq!(rx.try_recv().unwrap(), "gw.lo");

        // Right zone, wrong sender.
        assert!(!acceptor.accept("192.168.1.99".parse().unwrap(), "gw.lo"));
        // Right sender, zone we do not mirror.
        assert!(!acceptor.accept("192.168.1.252".parse().unwrap(), "g10.lo"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn an_ipv4_mapped_peer_is_still_the_primary() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let acceptor = NotifyAcceptor::new(&zones(), tx);
        assert!(acceptor.accept("::ffff:192.168.1.252".parse().unwrap(), "gw.lo"));
    }

    #[test]
    fn serials_compare_by_rfc1982_not_by_size() {
        assert!(is_newer(3, 2));
        assert!(!is_newer(2, 3));
        assert!(!is_newer(2, 2));
        // Wrapped: 1 is newer than u32::MAX.
        assert!(is_newer(1, u32::MAX));
        assert!(!is_newer(u32::MAX, 1));
    }
}
