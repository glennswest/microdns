use crate::AppState;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tracing::debug;

pub fn router() -> Router<AppState> {
    Router::new().route("/connectivity", get(connectivity_check))
}

#[derive(Serialize)]
struct ConnectivityResponse {
    instance_id: String,
    peers: Vec<PeerResult>,
}

#[derive(Serialize)]
struct PeerResult {
    id: String,
    addr: String,
    dns_udp: ProbeResult,
    dns_tcp: ProbeResult,
    http: ProbeResult,
}

#[derive(Serialize)]
struct ProbeResult {
    ok: bool,
    latency_ms: Option<f64>,
    error: Option<String>,
}

impl ProbeResult {
    fn success(latency: Duration) -> Self {
        Self {
            ok: true,
            latency_ms: Some(latency.as_secs_f64() * 1000.0),
            error: None,
        }
    }

    fn failure(err: String) -> Self {
        Self {
            ok: false,
            latency_ms: None,
            error: Some(err),
        }
    }
}

async fn connectivity_check(State(state): State<AppState>) -> Json<ConnectivityResponse> {
    let mut peers = Vec::new();

    for peer in &state.peers {
        let dns_addr: SocketAddr = format!("{}:{}", peer.addr, peer.dns_port)
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 53)));

        let http_url = format!("http://{}:{}/api/v1/health", peer.addr, peer.http_port);

        debug!("testing connectivity to peer {} ({})", peer.id, peer.addr);

        // Run all three probes concurrently
        let (dns_udp, dns_tcp, http) = tokio::join!(
            probe_dns_udp(dns_addr),
            probe_dns_tcp(dns_addr),
            probe_http(&http_url),
        );

        peers.push(PeerResult {
            id: peer.id.clone(),
            addr: peer.addr.clone(),
            dns_udp,
            dns_tcp,
            http,
        });
    }

    Json(ConnectivityResponse {
        instance_id: state.instance_id.clone(),
        peers,
    })
}

/// Build a minimal DNS query for "version.bind CH TXT" — a standard probe query.
fn build_probe_query() -> Vec<u8> {
    let mut buf = Vec::with_capacity(40);
    // Header: ID=0x1234, flags=RD, QDCOUNT=1
    buf.extend_from_slice(&[0x12, 0x34]); // ID
    buf.extend_from_slice(&[0x01, 0x00]); // flags: RD=1
    buf.extend_from_slice(&[0x00, 0x01]); // QDCOUNT=1
    buf.extend_from_slice(&[0x00, 0x00]); // ANCOUNT=0
    buf.extend_from_slice(&[0x00, 0x00]); // NSCOUNT=0
    buf.extend_from_slice(&[0x00, 0x00]); // ARCOUNT=0
    // Question: "." IN A (root query — minimal, always gets a response)
    buf.push(0x00); // root label
    buf.extend_from_slice(&[0x00, 0x01]); // QTYPE=A
    buf.extend_from_slice(&[0x00, 0x01]); // QCLASS=IN
    buf
}

async fn probe_dns_udp(addr: SocketAddr) -> ProbeResult {
    let timeout = Duration::from_secs(3);
    let start = Instant::now();

    let result = tokio::time::timeout(timeout, async {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        let query = build_probe_query();
        socket.send_to(&query, addr).await?;
        let mut buf = vec![0u8; 512];
        let (len, _) = socket.recv_from(&mut buf).await?;
        if len < 12 {
            anyhow::bail!("response too short: {} bytes", len);
        }
        Ok::<(), anyhow::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => ProbeResult::success(start.elapsed()),
        Ok(Err(e)) => ProbeResult::failure(e.to_string()),
        Err(_) => ProbeResult::failure("timeout (3s)".to_string()),
    }
}

async fn probe_dns_tcp(addr: SocketAddr) -> ProbeResult {
    let timeout = Duration::from_secs(3);
    let start = Instant::now();

    let result = tokio::time::timeout(timeout, async {
        let mut stream = TcpStream::connect(addr).await?;
        let query = build_probe_query();
        let len = query.len() as u16;
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&query).await?;
        stream.flush().await?;

        let resp_len = stream.read_u16().await? as usize;
        if resp_len < 12 {
            anyhow::bail!("response too short: {} bytes", resp_len);
        }
        let mut buf = vec![0u8; resp_len];
        stream.read_exact(&mut buf).await?;
        Ok::<(), anyhow::Error>(())
    })
    .await;

    match result {
        Ok(Ok(())) => ProbeResult::success(start.elapsed()),
        Ok(Err(e)) => ProbeResult::failure(e.to_string()),
        Err(_) => ProbeResult::failure("timeout (3s)".to_string()),
    }
}

/// Frame a DNS message for TCP transport (prepend 2-byte length).
pub fn frame_dns_tcp(msg: &[u8]) -> Vec<u8> {
    let len = msg.len() as u16;
    let mut framed = Vec::with_capacity(2 + msg.len());
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(msg);
    framed
}

async fn probe_http(url: &str) -> ProbeResult {
    let timeout = Duration::from_secs(3);
    let start = Instant::now();

    let result = tokio::time::timeout(timeout, async {
        // Use a raw TCP connection + minimal HTTP/1.1 request to avoid pulling in reqwest
        let url_parts: Vec<&str> = url
            .strip_prefix("http://")
            .unwrap_or(url)
            .splitn(2, '/')
            .collect();
        let host_port = url_parts[0];
        let path = if url_parts.len() > 1 {
            format!("/{}", url_parts[1])
        } else {
            "/".to_string()
        };

        let mut stream = TcpStream::connect(host_port).await?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await?;
        stream.flush().await?;

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await?;
        let response = String::from_utf8_lossy(&buf[..n]);

        // Check for HTTP 200
        if response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200") {
            Ok(())
        } else {
            let status_line = response.lines().next().unwrap_or("no response");
            anyhow::bail!("{}", status_line)
        }
    })
    .await;

    match result {
        Ok(Ok(())) => ProbeResult::success(start.elapsed()),
        Ok(Err(e)) => ProbeResult::failure(e.to_string()),
        Err(_) => ProbeResult::failure("timeout (3s)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_probe_query_structure() {
        let query = build_probe_query();
        // DNS header is 12 bytes, question section for "." IN A is 5 bytes
        assert_eq!(query.len(), 17);
        // ID
        assert_eq!(query[0], 0x12);
        assert_eq!(query[1], 0x34);
        // Flags: RD=1
        assert_eq!(query[2], 0x01);
        assert_eq!(query[3], 0x00);
        // QDCOUNT=1
        assert_eq!(query[4], 0x00);
        assert_eq!(query[5], 0x01);
        // Root label
        assert_eq!(query[12], 0x00);
        // QTYPE=A (1)
        assert_eq!(query[13], 0x00);
        assert_eq!(query[14], 0x01);
        // QCLASS=IN (1)
        assert_eq!(query[15], 0x00);
        assert_eq!(query[16], 0x01);
    }

    #[test]
    fn test_frame_dns_tcp() {
        let msg = vec![0x12, 0x34, 0x01, 0x00];
        let framed = frame_dns_tcp(&msg);
        assert_eq!(framed.len(), 6);
        // 2-byte length prefix = 4
        assert_eq!(framed[0], 0x00);
        assert_eq!(framed[1], 0x04);
        assert_eq!(&framed[2..], &msg);
    }

    #[test]
    fn test_probe_result_success() {
        let r = ProbeResult::success(Duration::from_millis(42));
        assert!(r.ok);
        assert!(r.latency_ms.unwrap() >= 42.0);
        assert!(r.error.is_none());
    }

    #[test]
    fn test_probe_result_failure() {
        let r = ProbeResult::failure("connection refused".to_string());
        assert!(!r.ok);
        assert!(r.latency_ms.is_none());
        assert_eq!(r.error.as_deref(), Some("connection refused"));
    }

    #[tokio::test]
    async fn test_probe_dns_udp_unreachable() {
        // Port 1 should be unreachable/timeout
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let result = probe_dns_udp(addr).await;
        // Should fail (timeout or error), not panic
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn test_probe_dns_tcp_unreachable() {
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let result = probe_dns_tcp(addr).await;
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn test_probe_http_unreachable() {
        let result = probe_http("http://127.0.0.1:1/health").await;
        assert!(!result.ok);
    }
}
