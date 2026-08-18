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
use std::time::Duration;
use tokio::net::UdpSocket;
use tracing::{debug, warn};

/// How long to wait for a secondary to acknowledge a NOTIFY.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);

/// Announce a zone change to a set of secondaries.
///
/// Targets are passed in per call rather than held here: they live in the
/// shared zone-transfer settings, which an operator can change through the API
/// while the server runs.
///
/// Spawned rather than awaited by callers on the write path: a slow or dead
/// secondary must never delay an API response.
pub async fn notify_zone(zone: &str, targets: &[SocketAddr]) {
    if targets.is_empty() {
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

    for target in targets {
        match send_notify(&wire, *target).await {
            Ok(()) => debug!("NOTIFY {zone} -> {target}"),
            // A lost NOTIFY only costs latency: the secondary still picks the
            // change up on its refresh timer.
            Err(e) => debug!(
                "NOTIFY {zone} -> {target} failed ({e}); \
                 secondary will catch up on its refresh timer"
            ),
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
    fn a_zone_name_that_cannot_be_parsed_is_not_announced() {
        // Exercised for the absence of a panic: a bad zone name in the database
        // must not take the announcer down.
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(notify_zone("..not a name..", &["127.0.0.1:1".parse().unwrap()]));
    }

    #[tokio::test]
    async fn notifying_with_no_targets_is_a_no_op() {
        notify_zone("gw.lo", &[]).await;
    }

    #[tokio::test]
    async fn a_dead_secondary_does_not_fail_the_announcement() {
        // Nothing is listening on this port; the call must still return.
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        notify_zone("gw.lo", &[dead]).await;
    }
}
