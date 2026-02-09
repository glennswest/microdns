use crate::catalog::ZoneCatalog;
use crate::transfer::ZoneTransfer;
use crate::zone;
use hickory_proto::op::{MessageType, OpCode, ResponseCode};
use hickory_proto::rr::{LowerName, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use microdns_core::db::Db;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tracing::{debug, error, info, warn};

pub struct AuthServer {
    listen_addr: SocketAddr,
    catalog: Arc<ZoneCatalog>,
    db: Db,
}

impl AuthServer {
    pub fn new(listen_addr: SocketAddr, db: Db) -> Self {
        Self {
            listen_addr,
            catalog: Arc::new(ZoneCatalog::new(db.clone())),
            db,
        }
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

        // TCP accept loop
        let tcp_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = tcp_listener.accept() => {
                        match result {
                            Ok((stream, src)) => {
                                debug!("TCP connection from {src}");
                                let catalog = catalog_tcp.clone();
                                let db = db_tcp.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = handle_tcp_connection(stream, &catalog, &db).await {
                                        warn!("TCP handler error from {src}: {e}");
                                    }
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

                    let response = Self::handle_query(&catalog, &data);
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

    fn handle_query(catalog: &ZoneCatalog, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        use hickory_proto::op::Message;

        let request = Message::from_bytes(data)?;

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
            response.set_response_code(ResponseCode::NXDomain);
        } else {
            for record in records {
                response.add_answer(record);
            }
            response.set_response_code(ResponseCode::NoError);
        }

        Ok(response.to_bytes()?)
    }
}

async fn handle_tcp_connection(
    mut stream: tokio::net::TcpStream,
    catalog: &ZoneCatalog,
    db: &Db,
) -> anyhow::Result<()> {
    // Read 2-byte length prefix
    let msg_len = stream.read_u16().await? as usize;
    if msg_len == 0 || msg_len > 65535 {
        return Ok(());
    }

    let mut buf = vec![0u8; msg_len];
    stream.read_exact(&mut buf).await?;

    let request = hickory_proto::op::Message::from_bytes(&buf)?;
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

        let zt = ZoneTransfer::new(db.clone());
        match zt.build_axfr_records(zone_name) {
            Ok(records) => {
                // Send records in a single response message per RFC 5936
                // (small zones fit in one message; large zones could be split)
                let mut response = hickory_proto::op::Message::new();
                response.set_id(request.id());
                response.set_message_type(MessageType::Response);
                response.set_op_code(OpCode::Query);
                response.set_authoritative(true);
                response.set_response_code(ResponseCode::NoError);

                for query in queries {
                    response.add_query(query.clone());
                }

                for record in records {
                    response.add_answer(record);
                }

                let wire = response.to_bytes()?;
                let len = wire.len() as u16;
                stream.write_all(&len.to_be_bytes()).await?;
                stream.write_all(&wire).await?;
                stream.flush().await?;
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
        let response = AuthServer::handle_query(catalog, &buf)?;
        let len = response.len() as u16;
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&response).await?;
        stream.flush().await?;
    }

    Ok(())
}
