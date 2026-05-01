use rand::random;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use surge_ping::{Client, Config, IcmpPacket, PingIdentifier, PingSequence, ICMP};
use tokio::sync::{Mutex as AsyncMutex, OnceCell};
use tracing::{debug, warn};

/// Lazily-initialized ICMP clients. Created once on first use and reused
/// for every probe. If the OS rejects raw-socket creation (typical when
/// `CAP_NET_RAW` is missing) we record that fact so the rest of the probe
/// loop falls back to the TCP-reachability stand-in.
static V4_CLIENT: OnceLock<AsyncMutex<Option<Client>>> = OnceLock::new();
static V6_CLIENT: OnceLock<AsyncMutex<Option<Client>>> = OnceLock::new();
static FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);
static FALLBACK_CELL: OnceCell<bool> = OnceCell::const_new();

fn v4_slot() -> &'static AsyncMutex<Option<Client>> {
    V4_CLIENT.get_or_init(|| AsyncMutex::new(None))
}
fn v6_slot() -> &'static AsyncMutex<Option<Client>> {
    V6_CLIENT.get_or_init(|| AsyncMutex::new(None))
}

async fn ensure_client(v6: bool) -> Option<Client> {
    let slot = if v6 { v6_slot() } else { v4_slot() };
    let mut g = slot.lock().await;
    if let Some(c) = g.as_ref() {
        return Some(c.clone());
    }
    let cfg = if v6 {
        Config::builder().kind(ICMP::V6).build()
    } else {
        Config::builder().kind(ICMP::V4).build()
    };
    match Client::new(&cfg) {
        Ok(c) => {
            *g = Some(c.clone());
            Some(c)
        }
        Err(e) => {
            // Cache a single warning so the log doesn't get hammered.
            if !FALLBACK_WARNED.swap(true, Ordering::Relaxed) {
                warn!(
                    "ICMP raw socket unavailable ({e}); ping probes will fall back to TCP-reachability stand-in. Add CAP_NET_RAW to enable real ICMP."
                );
            }
            None
        }
    }
}

/// True iff at least one IP family was able to open a raw ICMP socket.
/// Cached after the first call to keep the hot path branch-free.
pub async fn icmp_available() -> bool {
    *FALLBACK_CELL
        .get_or_init(|| async {
            let v4 = ensure_client(false).await.is_some();
            let v6 = ensure_client(true).await.is_some();
            v4 || v6
        })
        .await
}

/// Try a real-ICMP probe. Returns:
///   - `Some(Ok(detail))` on healthy response
///   - `Some(Err(detail))` on probe completion with no replies (timeout etc.)
///   - `None` when ICMP isn't available — caller should fall back to TCP.
pub async fn probe_if_available(
    target: IpAddr,
    timeout: Duration,
    count: u8,
) -> Option<Result<String, String>> {
    if !icmp_available().await {
        return None;
    }

    let client = match target {
        IpAddr::V4(_) => ensure_client(false).await?,
        IpAddr::V6(_) => ensure_client(true).await?,
    };

    let payload = [0u8; 56];
    let ident = PingIdentifier(random::<u16>());
    let mut pinger = client.pinger(target, ident).await;
    pinger.timeout(timeout);

    let n = count.max(1);
    let mut last_err = String::from("no reply");
    let per_packet_timeout = if n > 0 { timeout / u32::from(n) } else { timeout };
    let interval = Duration::from_millis(200).min(per_packet_timeout);

    for seq in 0..n {
        match pinger.ping(PingSequence(u16::from(seq)), &payload).await {
            Ok((IcmpPacket::V4(_pkt), rtt)) => {
                return Some(Ok(format!("icmp v4 reply in {:.1}ms", rtt.as_secs_f64() * 1000.0)));
            }
            Ok((IcmpPacket::V6(_pkt), rtt)) => {
                return Some(Ok(format!("icmp v6 reply in {:.1}ms", rtt.as_secs_f64() * 1000.0)));
            }
            Err(e) => {
                debug!("icmp seq={seq} to {target}: {e}");
                last_err = e.to_string();
            }
        }
        if seq + 1 < n {
            tokio::time::sleep(interval).await;
        }
    }
    Some(Err(format!("icmp: {last_err}")))
}
