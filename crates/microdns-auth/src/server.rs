use crate::catalog::ZoneCatalog;
use crate::transfer::ZoneTransfer;
use crate::secondary::NotifyAcceptor;
use crate::zone;
use hickory_proto::op::{MessageType, OpCode, ResponseCode};
use hickory_proto::rr::{LowerName, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use microdns_core::db::Db;
use microdns_core::query_tracker::QueryTracker;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

/// Maximum concurrent TCP connections
const MAX_TCP_CONNECTIONS: usize = 1000;

/// Timeout for TCP connection handling
const TCP_TIMEOUT: Duration = Duration::from_secs(30);

/// Records per AXFR message. RFC 5936 sends a zone as a sequence of messages;
/// this keeps each one far below the 65535-byte TCP framing limit even for
/// large records such as long TXT strings.
const AXFR_RECORDS_PER_MESSAGE: usize = 100;

pub struct AuthServer {
    listen_addr: SocketAddr,
    catalog: Arc<ZoneCatalog>,
    db: Db,
    tracker: Option<Arc<QueryTracker>>,
    /// CIDRs permitted to request a zone transfer. Empty denies everything.
    allow_transfer: Arc<Vec<IpNet>>,
    /// Accepts inbound NOTIFY for zones this instance mirrors. Absent when the
    /// instance is not a secondary for anything.
    notify: Option<NotifyAcceptor>,
}

impl AuthServer {
    pub fn new(listen_addr: SocketAddr, db: Db) -> Self {
        Self {
            listen_addr,
            catalog: Arc::new(ZoneCatalog::new(db.clone())),
            db,
            tracker: None,
            allow_transfer: Arc::new(Vec::new()),
            notify: None,
        }
    }

    /// Set the CIDRs permitted to request a zone transfer. Entries that do not
    /// parse are dropped with a warning rather than failing startup — a typo in
    /// an ACL must not take DNS down, and the effect is to deny, not permit.
    pub fn with_allow_transfer(mut self, cidrs: &[String]) -> Self {
        let nets: Vec<IpNet> = cidrs
            .iter()
            .filter_map(|c| match IpNet::parse(c) {
                Some(net) => Some(net),
                None => {
                    warn!("ignoring invalid allow_transfer entry '{c}'");
                    None
                }
            })
            .collect();
        if nets.is_empty() {
            warn!("allow_transfer is empty — all zone transfer requests will be refused");
        } else {
            info!("zone transfers permitted from: {}", cidrs.join(", "));
        }
        self.allow_transfer = Arc::new(nets);
        self
    }

    pub fn with_query_tracker(mut self, tracker: Arc<QueryTracker>) -> Self {
        self.tracker = Some(tracker);
        self
    }

    /// Accept inbound NOTIFY for the zones this instance mirrors, handing each
    /// accepted one to the secondary agent.
    pub fn with_notify_acceptor(mut self, acceptor: NotifyAcceptor) -> Self {
        self.notify = Some(acceptor);
        self
    }

    pub async fn run(self, shutdown: tokio::sync::watch::Receiver<bool>) -> anyhow::Result<()> {
        let socket = UdpSocket::bind(self.listen_addr).await?;
        let tcp_listener = TcpListener::bind(self.listen_addr).await?;
        info!(
            "auth DNS server listening on {} (UDP+TCP)",
            self.listen_addr
        );

        let mut buf = vec![0u8; 4096];
        let mut shutdown_udp = shutdown.clone();
        let mut shutdown_tcp = shutdown;

        let catalog_tcp = self.catalog.clone();
        let db_tcp = self.db.clone();
        let tracker_tcp = self.tracker.clone();
        let allow_tcp = self.allow_transfer.clone();
        let notify_tcp = self.notify.clone();

        // TCP accept loop with connection limit
        let tcp_semaphore = Arc::new(Semaphore::new(MAX_TCP_CONNECTIONS));
        let tcp_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = tcp_listener.accept() => {
                        match result {
                            Ok((stream, src)) => {
                                let permit = match tcp_semaphore.clone().try_acquire_owned() {
                                    Ok(p) => p,
                                    Err(_) => {
                                        warn!("TCP connection limit reached, rejecting {src}");
                                        continue;
                                    }
                                };
                                debug!("TCP connection from {src}");
                                let catalog = catalog_tcp.clone();
                                let db = db_tcp.clone();
                                let tracker = tracker_tcp.clone();
                                let allow = allow_tcp.clone();
                                let notify = notify_tcp.clone();
                                tokio::spawn(async move {
                                    let result = tokio::time::timeout(
                                        TCP_TIMEOUT,
                                        handle_tcp_connection(
                                            stream,
                                            &catalog,
                                            &db,
                                            tracker.as_deref(),
                                            src,
                                            &allow,
                                            notify.as_ref(),
                                        ),
                                    ).await;
                                    match result {
                                        Ok(Err(e)) => warn!("TCP handler error from {src}: {e}"),
                                        Err(_) => warn!("TCP handler timeout from {src}"),
                                        _ => {}
                                    }
                                    drop(permit);
                                });
                            }
                            Err(e) => {
                                error!("TCP accept error: {e}");
                            }
                        }
                    }
                    _ = shutdown_tcp.changed() => {
                        if *shutdown_tcp.borrow() {
                            break;
                        }
                    }
                }
            }
        });

        // UDP recv loop
        loop {
            tokio::select! {
                result = socket.recv_from(&mut buf) => {
                    let (len, src) = result?;
                    let data = buf[..len].to_vec();
                    let catalog = self.catalog.clone();
                    let socket_ref = &socket;

                    let response = Self::handle_query(
                        &catalog,
                        &data,
                        self.tracker.as_deref(),
                        src,
                        self.notify.as_ref(),
                    );
                    match response {
                        Ok(resp) => {
                            if let Err(e) = socket_ref.send_to(&resp, src).await {
                                error!("failed to send response to {src}: {e}");
                            }
                        }
                        Err(e) => {
                            warn!("failed to handle query from {src}: {e}");
                        }
                    }
                }
                _ = shutdown_udp.changed() => {
                    if *shutdown_udp.borrow() {
                        info!("auth DNS server shutting down");
                        break;
                    }
                }
            }
        }

        tcp_handle.abort();
        Ok(())
    }

    /// Answer an inbound NOTIFY (RFC 1996 §3).
    ///
    /// The reply is an acknowledgement that the message was received, not that
    /// a transfer happened — the sender is not made to wait for one. Anything
    /// this instance does not mirror, or that comes from an address other than
    /// that zone's configured primary, is refused.
    fn handle_notify(
        request: &hickory_proto::op::Message,
        peer: SocketAddr,
        notify: Option<&NotifyAcceptor>,
    ) -> anyhow::Result<Vec<u8>> {
        let mut response = hickory_proto::op::Message::new();
        response.set_id(request.id());
        response.set_message_type(MessageType::Response);
        response.set_op_code(OpCode::Notify);
        response.set_authoritative(true);
        for query in request.queries() {
            response.add_query(query.clone());
        }

        let Some(zone) = request.queries().first().map(|q| q.name().to_string()) else {
            response.set_response_code(ResponseCode::FormErr);
            return Ok(response.to_bytes()?);
        };

        match notify {
            Some(acceptor) if acceptor.accept(peer.ip(), &zone) => {
                info!("NOTIFY accepted for {zone} from {peer}");
                response.set_response_code(ResponseCode::NoError);
            }
            Some(_) => {
                warn!("NOTIFY for {zone} refused: {peer} is not the configured primary");
                response.set_response_code(ResponseCode::Refused);
            }
            None => {
                debug!("NOTIFY for {zone} from {peer} ignored: not a secondary for any zone");
                response.set_response_code(ResponseCode::Refused);
            }
        }
        Ok(response.to_bytes()?)
    }

    fn handle_query(
        catalog: &ZoneCatalog,
        data: &[u8],
        tracker: Option<&QueryTracker>,
        peer: SocketAddr,
        notify: Option<&NotifyAcceptor>,
    ) -> anyhow::Result<Vec<u8>> {
        use hickory_proto::op::Message;

        let request = Message::from_bytes(data)?;

        if request.op_code() == OpCode::Notify {
            return Self::handle_notify(&request, peer, notify);
        }

        let mut response = Message::new();
        response.set_id(request.id());
        response.set_message_type(MessageType::Response);
        response.set_op_code(OpCode::Query);
        response.set_recursion_desired(request.recursion_desired());
        response.set_recursion_available(false);
        response.set_authoritative(true);

        if request.op_code() != OpCode::Query {
            response.set_response_code(ResponseCode::NotImp);
            return Ok(response.to_bytes()?);
        }

        let queries = request.queries();
        if queries.is_empty() {
            response.set_response_code(ResponseCode::FormErr);
            return Ok(response.to_bytes()?);
        }

        // Copy the query section
        for query in queries {
            response.add_query(query.clone());
        }

        let query = &queries[0];
        let qname: LowerName = LowerName::from(query.name().clone());
        let qtype = query.query_type();

        // Bump the per-(fqdn,type) query tracker before we even decide
        // authoritativeness. This counts every received query — useful
        // for spotting stale entries that nothing actually resolves.
        if let Some(tracker) = tracker {
            if let Some(rtype) = map_hickory_type(qtype) {
                let fqdn = qname.to_string();
                let fqdn = fqdn.trim_end_matches('.');
                tracker.bump(fqdn, rtype, chrono::Utc::now());
            }
        }

        debug!("query: {} {} from catalog", qname, qtype);

        // Check if we're authoritative for this zone
        if !catalog.is_authoritative(&qname) {
            response.set_response_code(ResponseCode::Refused);
            return Ok(response.to_bytes()?);
        }

        // Handle ANY queries
        if qtype == RecordType::ANY {
            let records = zone::resolve_query(catalog.db(), &qname, RecordType::SOA);
            for record in records {
                response.add_answer(record);
            }
            response.set_response_code(ResponseCode::NoError);
            return Ok(response.to_bytes()?);
        }

        let records = zone::resolve_query(catalog.db(), &qname, qtype);

        if records.is_empty() {
            if let Some(soa) = zone::get_authority_soa(catalog.db(), &qname) {
                response.add_name_server(soa);
            }
            // Check if the name exists with other record types.
            // NXDOMAIN = name doesn't exist at all; NOERROR = name exists but
            // no records of the queried type (critical for systemd-resolved
            // which does parallel A+AAAA lookups).
            let fqdn_str = qname.to_string();
            let fqdn_str = fqdn_str.trim_end_matches('.');
            let name_exists = catalog.db().fqdn_exists(fqdn_str).unwrap_or(false);
            if name_exists {
                response.set_response_code(ResponseCode::NoError);
            } else {
                response.set_response_code(ResponseCode::NXDomain);
            }
        } else {
            for record in records {
                response.add_answer(record);
            }
            response.set_response_code(ResponseCode::NoError);
        }

        Ok(response.to_bytes()?)
    }
}

/// Map hickory's `RecordType` to our core `RecordType` for tracker keys.
/// Returns `None` for types we don't model (ANY, AXFR, OPT, etc.).
fn map_hickory_type(t: RecordType) -> Option<microdns_core::types::RecordType> {
    use microdns_core::types::RecordType as CRT;
    Some(match t {
        RecordType::A => CRT::A,
        RecordType::AAAA => CRT::AAAA,
        RecordType::CNAME => CRT::CNAME,
        RecordType::MX => CRT::MX,
        RecordType::NS => CRT::NS,
        RecordType::PTR => CRT::PTR,
        RecordType::SOA => CRT::SOA,
        RecordType::SRV => CRT::SRV,
        RecordType::TXT => CRT::TXT,
        RecordType::CAA => CRT::CAA,
        _ => return None,
    })
}

async fn handle_tcp_connection(
    mut stream: tokio::net::TcpStream,
    catalog: &ZoneCatalog,
    db: &Db,
    tracker: Option<&QueryTracker>,
    peer: SocketAddr,
    allow_transfer: &[IpNet],
    notify: Option<&NotifyAcceptor>,
) -> anyhow::Result<()> {
    // Read 2-byte length prefix
    let msg_len = stream.read_u16().await? as usize;
    if msg_len == 0 || msg_len > 65535 {
        return Ok(());
    }

    let mut buf = vec![0u8; msg_len];
    stream.read_exact(&mut buf).await?;

    let request = hickory_proto::op::Message::from_bytes(&buf)?;

    // A NOTIFY may arrive over TCP as readily as UDP.
    if request.op_code() == OpCode::Notify {
        let wire = AuthServer::handle_notify(&request, peer, notify)?;
        stream.write_all(&(wire.len() as u16).to_be_bytes()).await?;
        stream.write_all(&wire).await?;
        stream.flush().await?;
        return Ok(());
    }

    let queries = request.queries();
    if queries.is_empty() {
        return Ok(());
    }

    let qtype = queries[0].query_type();

    if qtype == RecordType::AXFR {
        // Handle AXFR
        let qname = queries[0].name().to_string();
        let zone_name = qname.trim_end_matches('.');
        debug!("AXFR request for {zone_name}");

        // A zone transfer hands over a complete map of internal hosts, so the
        // peer must be explicitly permitted before we build anything.
        if !transfer_allowed(peer, allow_transfer) {
            warn!("AXFR for {zone_name} refused: {peer} is not in allow_transfer");
            let wire = refusal(&request, queries)?;
            stream.write_all(&(wire.len() as u16).to_be_bytes()).await?;
            stream.write_all(&wire).await?;
            stream.flush().await?;
            return Ok(());
        }

        let zt = ZoneTransfer::new(db.clone());
        match zt.build_axfr_records(zone_name) {
            Ok(records) => {
                // RFC 5936 §2.2: a zone is sent as a *sequence* of messages.
                // Packing every record into one message caps the zone at the
                // 64 KB TCP length prefix, and `len as u16` would wrap silently
                // rather than error — a corrupt transfer that is very hard to
                // diagnose. Chunking keeps each message well under the limit.
                let total = records.len();
                let mut sent = 0;

                for chunk in records.chunks(AXFR_RECORDS_PER_MESSAGE) {
                    let mut response = hickory_proto::op::Message::new();
                    response.set_id(request.id());
                    response.set_message_type(MessageType::Response);
                    response.set_op_code(OpCode::Query);
                    response.set_authoritative(true);
                    response.set_response_code(ResponseCode::NoError);

                    for query in queries {
                        response.add_query(query.clone());
                    }
                    for record in chunk {
                        response.add_answer(record.clone());
                    }

                    let wire = response.to_bytes()?;
                    if wire.len() > u16::MAX as usize {
                        return Err(anyhow::anyhow!(
                            "AXFR message for {zone_name} exceeded 65535 bytes even when \
                             chunked; lower AXFR_RECORDS_PER_MESSAGE"
                        ));
                    }
                    stream.write_all(&(wire.len() as u16).to_be_bytes()).await?;
                    stream.write_all(&wire).await?;
                    sent += chunk.len();
                }

                stream.flush().await?;
                info!("AXFR {zone_name} -> {peer}: sent {sent}/{total} records");
            }
            Err(e) => {
                warn!("AXFR failed for {zone_name}: {e}");
                let mut response = hickory_proto::op::Message::new();
                response.set_id(request.id());
                response.set_message_type(MessageType::Response);
                response.set_op_code(OpCode::Query);
                response.set_response_code(ResponseCode::Refused);
                for query in queries {
                    response.add_query(query.clone());
                }
                let wire = response.to_bytes()?;
                let len = wire.len() as u16;
                stream.write_all(&len.to_be_bytes()).await?;
                stream.write_all(&wire).await?;
                stream.flush().await?;
            }
        }
    } else {
        // Regular TCP query — reuse UDP handler
        let response = AuthServer::handle_query(catalog, &buf, tracker, peer, notify)?;
        let len = response.len() as u16;
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&response).await?;
        stream.flush().await?;
    }

    Ok(())
}

/// A CIDR block, kept local so the ACL adds no dependency.
#[derive(Debug, Clone, Copy)]
pub struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

impl IpNet {
    /// Parse `10.0.0.0/8`, or a bare address treated as a single host.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (addr_part, prefix_part) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = addr_part.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix_part {
            Some(p) => p.parse::<u8>().ok()?,
            None => max,
        };
        if prefix > max {
            return None;
        }
        Some(Self { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(other)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix) };
                (u32::from(net) & mask) == (u32::from(other) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(other)) => {
                let mask = if self.prefix == 0 {
                    0u128
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                (u128::from(net) & mask) == (u128::from(other) & mask)
            }
            // An IPv4-mapped IPv6 peer should still match an IPv4 rule.
            (IpAddr::V4(_), IpAddr::V6(other)) => other
                .to_ipv4_mapped()
                .is_some_and(|v4| self.contains(IpAddr::V4(v4))),
            (IpAddr::V6(_), IpAddr::V4(_)) => false,
        }
    }
}

/// Whether a peer may request a zone transfer. Denies by default.
fn transfer_allowed(peer: SocketAddr, allow: &[IpNet]) -> bool {
    allow.iter().any(|net| net.contains(peer.ip()))
}

/// Build a REFUSED response for a request we will not serve.
fn refusal(
    request: &hickory_proto::op::Message,
    queries: &[hickory_proto::op::Query],
) -> anyhow::Result<Vec<u8>> {
    let mut response = hickory_proto::op::Message::new();
    response.set_id(request.id());
    response.set_message_type(MessageType::Response);
    response.set_op_code(OpCode::Query);
    response.set_response_code(ResponseCode::Refused);
    for query in queries {
        response.add_query(query.clone());
    }
    Ok(response.to_bytes()?)
}

#[cfg(test)]
mod acl_tests {
    use super::*;

    #[test]
    fn cidr_matching() {
        let net = IpNet::parse("192.168.0.0/16").unwrap();
        assert!(net.contains("192.168.1.253".parse().unwrap()));
        assert!(net.contains("192.168.200.199".parse().unwrap()));
        assert!(!net.contains("10.0.0.1".parse().unwrap()));

        // A bare address is a single host.
        let host = IpNet::parse("192.168.1.253").unwrap();
        assert!(host.contains("192.168.1.253".parse().unwrap()));
        assert!(!host.contains("192.168.1.254".parse().unwrap()));

        assert!(IpNet::parse("192.168.0.0/33").is_none());
        assert!(IpNet::parse("not-an-address").is_none());
    }

    #[test]
    fn transfers_are_denied_by_default() {
        let peer: SocketAddr = "192.168.1.253:5000".parse().unwrap();
        // Empty ACL denies everything — a missing config must not expose zones.
        assert!(!transfer_allowed(peer, &[]));

        let allow = vec![IpNet::parse("192.168.0.0/16").unwrap()];
        assert!(transfer_allowed(peer, &allow));

        let outside: SocketAddr = "203.0.113.5:5000".parse().unwrap();
        assert!(!transfer_allowed(outside, &allow));
    }

    #[test]
    fn ipv4_mapped_peers_match_ipv4_rules() {
        // A dual-stack listener reports IPv4 peers as ::ffff:a.b.c.d; without
        // this the ACL would silently refuse every legitimate secondary.
        let allow = vec![IpNet::parse("192.168.0.0/16").unwrap()];
        let mapped: SocketAddr = "[::ffff:192.168.1.253]:5000".parse().unwrap();
        assert!(transfer_allowed(mapped, &allow));
    }
}
