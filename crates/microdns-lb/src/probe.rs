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

/// Execute a health check probe against a target IP.
pub async fn run_probe(
    probe_type: ProbeType,
    target: IpAddr,
    timeout: Duration,
    endpoint: Option<&str>,
) -> ProbeResult {
    let start = std::time::Instant::now();

    let result = match probe_type {
        ProbeType::Ping => ping_probe(target, timeout).await,
        ProbeType::Http => http_probe(target, false, timeout, endpoint).await,
        ProbeType::Https => http_probe(target, true, timeout, endpoint).await,
        ProbeType::Tcp => tcp_probe(target, timeout, endpoint).await,
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

/// ICMP ping probe - uses TCP connect to port 7 as a fallback since raw sockets
/// require privileges. In production with NET_RAW capability, this could use
/// actual ICMP. For now we use a TCP connect to a common port as a reachability check.
async fn ping_probe(target: IpAddr, timeout: Duration) -> Result<String, String> {
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
    // Parse endpoint for port if provided (e.g., ":8080/health")
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

/// TCP connect probe - checks if a TCP connection can be established.
async fn tcp_probe(
    target: IpAddr,
    timeout: Duration,
    endpoint: Option<&str>,
) -> Result<String, String> {
    let port: u16 = endpoint
        .and_then(|ep| ep.trim_start_matches(':').parse().ok())
        .unwrap_or(80);

    let addr = SocketAddr::new(target, port);
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(_)) => Ok(format!("tcp/{port} connected")),
        Ok(Err(e)) => Err(format!("tcp/{port}: {e}")),
        Err(_) => Err(format!("tcp/{port}: timeout")),
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
        )
        .await;
        // Should fail (connection refused)
        assert!(result.is_err());
    }
}
