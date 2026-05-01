use microdns_core::types::ProbeType;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::TcpStream;
use tracing::{debug, warn};

/// Result of a single probe execution.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub success: bool,
    pub latency: Duration,
    pub detail: String,
}

/// Execute a health check probe against a target IP. `ping_count` is the
/// number of ICMP echos to send for ping probes (ignored otherwise).
pub async fn run_probe(
    probe_type: ProbeType,
    target: IpAddr,
    timeout: Duration,
    endpoint: Option<&str>,
    ping_count: u8,
) -> ProbeResult {
    let start = std::time::Instant::now();

    let result = match probe_type {
        ProbeType::Ping => ping_probe(target, timeout, ping_count).await,
        ProbeType::Http => http_probe(target, false, timeout, endpoint).await,
        ProbeType::Https => http_probe(target, true, timeout, endpoint).await,
        ProbeType::Tcp => tcp_probe(target, timeout, endpoint, false).await,
        ProbeType::TcpHalfOpen => tcp_probe(target, timeout, endpoint, true).await,
    };

    let latency = start.elapsed();

    match result {
        Ok(detail) => {
            debug!("probe {probe_type:?} to {target}: OK ({detail}) in {latency:?}");
            ProbeResult {
                success: true,
                latency,
                detail,
            }
        }
        Err(e) => {
            warn!("probe {probe_type:?} to {target}: FAIL ({e}) in {latency:?}");
            ProbeResult {
                success: false,
                latency,
                detail: e.to_string(),
            }
        }
    }
}

/// Ping probe. With CAP_NET_RAW we use real ICMP via `surge_ping` (see
/// `icmp_probe` in `icmp.rs`). Without it we fall back to a TCP-reachability
/// stand-in: try TCP/80, then TCP/443. A connection-refused on either is
/// treated as "host reachable" since it proves the IP stack responded.
async fn ping_probe(target: IpAddr, timeout: Duration, count: u8) -> Result<String, String> {
    if let Some(result) = crate::icmp::probe_if_available(target, timeout, count).await {
        return result;
    }
    // Try TCP connect to port 80 as a reachability check
    // Real ICMP requires raw sockets / CAP_NET_RAW
    let addr = SocketAddr::new(target, 80);
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(_)) => Ok("tcp/80 reachable".to_string()),
        Ok(Err(_)) => {
            // Connection refused means host is up but port closed - that's still "reachable"
            // Try port 443 as fallback
            let addr443 = SocketAddr::new(target, 443);
            match tokio::time::timeout(timeout, TcpStream::connect(addr443)).await {
                Ok(Ok(_)) => Ok("tcp/443 reachable".to_string()),
                Ok(Err(e)) => {
                    // Connection refused = host is reachable
                    if e.kind() == std::io::ErrorKind::ConnectionRefused {
                        Ok("host reachable (connection refused)".to_string())
                    } else {
                        Err(format!("unreachable: {e}"))
                    }
                }
                Err(_) => Err("timeout".to_string()),
            }
        }
        Err(_) => Err("timeout".to_string()),
    }
}

/// HTTP/HTTPS probe - makes a GET request and checks for 2xx status.
async fn http_probe(
    target: IpAddr,
    https: bool,
    timeout: Duration,
    endpoint: Option<&str>,
) -> Result<String, String> {
    let scheme = if https { "https" } else { "http" };
    // Parse endpoint for port if provided (e.g., ":80/health")
    let (port_str, actual_path) = if let Some(ep) = endpoint {
        if let Some(rest) = ep.strip_prefix(':') {
            if let Some(slash_pos) = rest.find('/') {
                (
                    &rest[..slash_pos],
                    &rest[slash_pos..],
                )
            } else {
                (rest, "/")
            }
        } else {
            ("", ep)
        }
    } else {
        ("", "/")
    };

    let url = if port_str.is_empty() {
        format!("{scheme}://{target}{actual_path}")
    } else {
        format!("{scheme}://{target}:{port_str}{actual_path}")
    };

    let client = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(true) // Health checks don't validate certs
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    if status.is_success() {
        Ok(format!("HTTP {status}"))
    } else {
        Err(format!("HTTP {status}"))
    }
}

/// TCP connect probe.
///
/// `half_open=false`: standard connect + drop. Closes gracefully (FIN/ACK).
///
/// `half_open=true`: completes the SYN/SYN-ACK handshake, then sets
/// `SO_LINGER=0` so dropping the socket sends RST instead of FIN. The
/// backend sees only the SYN/SYN-ACK exchange and a reset — no
/// application-level connection is established, no half-open accept
/// queue entry survives, and the prober avoids leaving a TIME_WAIT
/// behind. Useful for high-frequency probes against services that
/// would otherwise log every check as a real client.
async fn tcp_probe(
    target: IpAddr,
    timeout: Duration,
    endpoint: Option<&str>,
    half_open: bool,
) -> Result<String, String> {
    let port: u16 = endpoint
        .and_then(|ep| ep.trim_start_matches(':').parse().ok())
        .unwrap_or(80);

    let addr = SocketAddr::new(target, port);
    let label = if half_open { "tcp-half-open" } else { "tcp" };
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => {
            if half_open {
                // SO_LINGER l_linger=0 → kernel sends RST on close. Set via
                // socket2::SockRef because tokio deprecated set_linger over
                // generic blocking concerns; with l_linger=0 the close is
                // non-blocking (RST is queued and sent immediately), which
                // is exactly the case we want.
                let sock = socket2::SockRef::from(&stream);
                if let Err(e) = sock.set_linger(Some(Duration::ZERO)) {
                    return Err(format!("{label}/{port}: set_linger failed: {e}"));
                }
            }
            // Drop closes the socket. With linger=0 this is RST; otherwise FIN.
            drop(stream);
            Ok(format!("{label}/{port} connected"))
        }
        Ok(Err(e)) => Err(format!("{label}/{port}: {e}")),
        Err(_) => Err(format!("{label}/{port}: timeout")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tcp_probe_refused() {
        // Connecting to a port that's almost certainly closed
        let result = tcp_probe(
            "127.0.0.1".parse().unwrap(),
            Duration::from_secs(1),
            Some(":19999"),
            false,
        )
        .await;
        // Should fail (connection refused)
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tcp_half_open_against_listener() {
        // Stand up a one-shot listener on a free port, probe it half-open,
        // and verify the probe succeeds. We don't try to assert on the wire
        // (RST is hard to observe portably from userspace) but we *do* prove
        // the SO_LINGER=0 path runs without erroring.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            // Accept and immediately drop — we don't care what the prober sends
            let _ = listener.accept().await;
        });

        let endpoint = format!(":{}", addr.port());
        let result = tcp_probe(
            addr.ip(),
            Duration::from_secs(2),
            Some(&endpoint),
            true,
        )
        .await;

        assert!(result.is_ok(), "half-open probe should succeed: {result:?}");
        let _ = accept.await;
    }
}
