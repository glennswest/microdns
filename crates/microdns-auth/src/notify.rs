//! Outbound DNS NOTIFY (RFC 1996).
//!
//! Without NOTIFY a secondary only discovers a change when its SOA refresh
//! timer fires. Every zone here uses a 3600 s refresh, so a record or
//! reservation added during an incident could take an hour to reach the
//! fallback server — long enough that the fallback would be serving visibly
//! wrong data at exactly the moment it is relied on.
//!
//! NOTIFY collapses that to seconds: the primary tells each secondary the zone
//! changed, and the secondary immediately checks the SOA and transfers if
//! needed. It is a hint, not a delivery guarantee — a lost NOTIFY simply means
//! the secondary falls back to its refresh timer, so failures here are logged
//! and never block a write.

use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::{Name, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tracing::{debug, warn};

/// How long to wait for a secondary to acknowledge a NOTIFY.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);

/// Sends NOTIFY to a fixed set of secondaries.
#[derive(Clone)]
pub struct Notifier {
    targets: Arc<Vec<SocketAddr>>,
}

impl Notifier {
    /// Build from configured `ip` or `ip:port` entries. Unparseable entries are
    /// dropped with a warning rather than failing startup — a malformed
    /// secondary address must not stop the server from serving DNS.
    pub fn new(targets: &[String]) -> Self {
        let parsed = targets
            .iter()
            .filter_map(|t| match parse_target(t) {
                Some(addr) => Some(addr),
                None => {
                    warn!("ignoring invalid notify target '{t}'");
                    None
                }
            })
            .collect::<Vec<_>>();
        Self {
            targets: Arc::new(parsed),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub fn targets(&self) -> &[SocketAddr] {
        &self.targets
    }

    /// Notify every secondary that a zone changed.
    ///
    /// Spawned rather than awaited by callers on the write path: a slow or dead
    /// secondary must never delay an API response.
    pub async fn notify_zone(&self, zone: &str) {
        if self.targets.is_empty() {
            return;
        }
        let Ok(name) = Name::from_str(&ensure_fqdn(zone)) else {
            warn!("cannot NOTIFY for unparseable zone name '{zone}'");
            return;
        };

        let mut query = Query::new();
        query.set_name(name);
        query.set_query_type(RecordType::SOA);

        let mut message = Message::new();
        message.set_id(notify_id());
        message.set_message_type(MessageType::Query);
        message.set_op_code(OpCode::Notify);
        message.set_authoritative(true);
        message.add_query(query);

        let Ok(wire) = message.to_bytes() else {
            return;
        };

        for target in self.targets.iter() {
            match send_notify(&wire, *target).await {
                Ok(()) => debug!("NOTIFY {zone} -> {target}"),
                // A lost NOTIFY only costs latency: the secondary still picks
                // the change up on its refresh timer.
                Err(e) => debug!("NOTIFY {zone} -> {target} failed ({e}); \
                                  secondary will catch up on its refresh timer"),
            }
        }
    }
}

async fn send_notify(wire: &[u8], target: SocketAddr) -> anyhow::Result<()> {
    let bind: SocketAddr = if target.is_ipv4() {
        "0.0.0.0:0".parse()?
    } else {
        "[::]:0".parse()?
    };
    let socket = UdpSocket::bind(bind).await?;
    socket.send_to(wire, target).await?;

    // Wait for the acknowledgement so a refusing secondary is visible in logs,
    // but treat a timeout as non-fatal.
    let mut buf = vec![0u8; 512];
    match tokio::time::timeout(NOTIFY_TIMEOUT, socket.recv(&mut buf)).await {
        Ok(Ok(len)) => {
            if let Ok(response) = Message::from_bytes(&buf[..len]) {
                if response.response_code() != hickory_proto::op::ResponseCode::NoError {
                    anyhow::bail!("secondary replied {}", response.response_code());
                }
            }
            Ok(())
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_) => anyhow::bail!("no response within {NOTIFY_TIMEOUT:?}"),
    }
}

/// Parse `192.168.1.253` or `192.168.1.253:53`, defaulting to port 53.
fn parse_target(target: &str) -> Option<SocketAddr> {
    let t = target.trim();
    if let Ok(addr) = t.parse::<SocketAddr>() {
        return Some(addr);
    }
    if let Ok(ip) = t.parse::<std::net::IpAddr>() {
        return Some(SocketAddr::new(ip, 53));
    }
    None
}

fn ensure_fqdn(name: &str) -> String {
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{name}.")
    }
}

fn notify_id() -> u16 {
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

    #[test]
    fn parses_notify_targets() {
        assert_eq!(
            parse_target("192.168.1.253"),
            Some("192.168.1.253:53".parse().unwrap())
        );
        assert_eq!(
            parse_target("192.168.1.253:5353"),
            Some("192.168.1.253:5353".parse().unwrap())
        );
        assert_eq!(parse_target("  192.168.1.253  "), Some("192.168.1.253:53".parse().unwrap()));
        assert_eq!(parse_target("ns1.gw.lo"), None);
    }

    #[test]
    fn invalid_targets_are_dropped_not_fatal() {
        let notifier = Notifier::new(&[
            "192.168.1.253".to_string(),
            "nonsense".to_string(),
            "192.168.8.253:53".to_string(),
        ]);
        assert_eq!(notifier.targets().len(), 2);
    }

    #[tokio::test]
    async fn notifying_with_no_targets_is_a_no_op() {
        let notifier = Notifier::new(&[]);
        assert!(notifier.is_empty());
        notifier.notify_zone("gw.lo").await;
    }
}
