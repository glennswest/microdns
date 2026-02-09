use crate::cache::{self, CacheKey, DnsCache};
use crate::forward::ForwardTable;
use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::{LowerName, Name, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use microdns_core::db::Db;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, warn};

/// The recursive resolver. Handles incoming queries by:
/// 1. Checking local authoritative zones (if db is provided)
/// 2. Checking the cache
/// 3. Forwarding to upstream servers (forward zones or general recursion)
pub struct Resolver {
    cache: Arc<DnsCache>,
    forward_table: Arc<ForwardTable>,
    db: Option<Db>,
    /// Upstream resolvers for general recursion (e.g., 8.8.8.8, 1.1.1.1)
    upstream: Vec<SocketAddr>,
}

impl Resolver {
    pub fn new(
        cache: Arc<DnsCache>,
        forward_table: Arc<ForwardTable>,
        db: Option<Db>,
    ) -> Self {
        // Default upstream resolvers
        let upstream = vec![
            "8.8.8.8:53".parse().unwrap(),
            "8.8.4.4:53".parse().unwrap(),
            "1.1.1.1:53".parse().unwrap(),
        ];

        Self {
            cache,
            forward_table,
            db,
            upstream,
        }
    }

    /// Resolve a DNS query from raw bytes. Returns the response bytes.
    pub async fn resolve(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let request = Message::from_bytes(data)?;

        if request.op_code() != OpCode::Query {
            return Ok(self.make_error_response(&request, ResponseCode::NotImp));
        }

        let queries = request.queries();
        if queries.is_empty() {
            return Ok(self.make_error_response(&request, ResponseCode::FormErr));
        }

        let query = &queries[0];
        let qname = query.name();
        let qtype = query.query_type();
        let qname_str = qname.to_string();
        let qname_lower = qname_str.trim_end_matches('.').to_lowercase();

        debug!("recursor query: {} {}", qname, qtype);

        // Step 1: Check local authoritative zones
        if let Some(ref db) = self.db {
            let lower = LowerName::from(qname.clone());
            if let Ok(Some(_zone)) = db.find_zone_for_fqdn(&qname_lower) {
                debug!("resolving {} {} from local auth zone", qname, qtype);
                return self.resolve_from_local(db, &request, &lower, qtype);
            }
        }

        // Step 2: Check cache
        let cache_key = CacheKey::from_query(
            &qname_lower,
            qtype.into(),
            query.query_class().into(),
        );

        if let Some(cached_bytes) = self.cache.get(&cache_key) {
            debug!("cache hit for {} {}", qname, qtype);
            // Rewrite the response ID to match the request
            return Ok(self.rewrite_response_id(&cached_bytes, request.id()));
        }

        // Step 3: Check forward zones
        if let Some(servers) = self.forward_table.lookup(&qname_lower) {
            debug!("forwarding {} {} to forward zone servers", qname, qtype);
            return self.forward_query(data, &request, servers, &cache_key).await;
        }

        // Step 4: Forward to upstream resolvers
        debug!("forwarding {} {} to upstream resolvers", qname, qtype);
        self.forward_query(data, &request, &self.upstream, &cache_key).await
    }

    /// Resolve from local authoritative zone data.
    fn resolve_from_local(
        &self,
        db: &Db,
        request: &Message,
        qname: &LowerName,
        qtype: RecordType,
    ) -> anyhow::Result<Vec<u8>> {
        use microdns_core::types::RecordType as MicroRecordType;

        let mut response = Message::new();
        response.set_id(request.id());
        response.set_message_type(MessageType::Response);
        response.set_op_code(OpCode::Query);
        response.set_recursion_desired(request.recursion_desired());
        response.set_recursion_available(true);
        response.set_authoritative(true);

        for query in request.queries() {
            response.add_query(query.clone());
        }

        let fqdn = qname.to_string();
        let fqdn = fqdn.trim_end_matches('.');

        // Handle SOA queries
        if qtype == RecordType::SOA {
            if let Ok(Some(zone)) = db.find_zone_for_fqdn(fqdn) {
                if let Some(soa) = build_soa_record_proto(&zone) {
                    response.add_answer(soa);
                }
            }
            response.set_response_code(ResponseCode::NoError);
            return Ok(response.to_bytes()?);
        }

        // Convert to our type
        let micro_rtype = match qtype {
            RecordType::A => Some(MicroRecordType::A),
            RecordType::AAAA => Some(MicroRecordType::AAAA),
            RecordType::CNAME => Some(MicroRecordType::CNAME),
            RecordType::MX => Some(MicroRecordType::MX),
            RecordType::NS => Some(MicroRecordType::NS),
            RecordType::PTR => Some(MicroRecordType::PTR),
            RecordType::SRV => Some(MicroRecordType::SRV),
            RecordType::TXT => Some(MicroRecordType::TXT),
            RecordType::CAA => Some(MicroRecordType::CAA),
            _ => None,
        };

        if let Some(rtype) = micro_rtype {
            let records = db.query_fqdn(fqdn, rtype).unwrap_or_default();
            if records.is_empty() {
                // NXDOMAIN with SOA authority
                if let Ok(Some(zone)) = db.find_zone_for_fqdn(fqdn) {
                    if let Some(soa) = build_soa_record_proto(&zone) {
                        response.add_name_server(soa);
                    }
                }
                response.set_response_code(ResponseCode::NXDomain);
            } else {
                for record in &records {
                    if let Some(dns_record) = record_to_proto(record, db) {
                        response.add_answer(dns_record);
                    }
                }
                response.set_response_code(ResponseCode::NoError);
            }
        } else {
            response.set_response_code(ResponseCode::NotImp);
        }

        Ok(response.to_bytes()?)
    }

    /// Forward a query to upstream servers and cache the result.
    async fn forward_query(
        &self,
        raw_request: &[u8],
        request: &Message,
        servers: &[SocketAddr],
        cache_key: &CacheKey,
    ) -> anyhow::Result<Vec<u8>> {
        // Try each server in order
        for server in servers {
            match self.send_query(raw_request, *server).await {
                Ok(response_bytes) => {
                    // Cache the response
                    if let Ok(resp_msg) = Message::from_bytes(&response_bytes) {
                        let ttl = cache::min_ttl_from_response(&resp_msg);
                        if ttl > 0 && resp_msg.response_code() == ResponseCode::NoError {
                            self.cache.insert(
                                cache_key.clone(),
                                response_bytes.clone(),
                                ttl,
                            );
                        }
                    }

                    // Rewrite response ID to match request
                    return Ok(self.rewrite_response_id(&response_bytes, request.id()));
                }
                Err(e) => {
                    warn!("upstream {} failed: {}", server, e);
                    continue;
                }
            }
        }

        // All upstreams failed
        Ok(self.make_error_response(request, ResponseCode::ServFail))
    }

    /// Send a raw DNS query to a server and return the response bytes.
    async fn send_query(
        &self,
        data: &[u8],
        server: SocketAddr,
    ) -> anyhow::Result<Vec<u8>> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.send_to(data, server).await?;

        let mut buf = vec![0u8; 4096];
        let timeout = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            socket.recv_from(&mut buf),
        )
        .await??;

        Ok(buf[..timeout.0].to_vec())
    }

    /// Rewrite the ID field in a DNS response to match a different request ID.
    fn rewrite_response_id(&self, response: &[u8], new_id: u16) -> Vec<u8> {
        if response.len() < 2 {
            return response.to_vec();
        }
        let mut result = response.to_vec();
        let id_bytes = new_id.to_be_bytes();
        result[0] = id_bytes[0];
        result[1] = id_bytes[1];
        result
    }

    /// Build an error response message.
    fn make_error_response(&self, request: &Message, code: ResponseCode) -> Vec<u8> {
        let mut response = Message::new();
        response.set_id(request.id());
        response.set_message_type(MessageType::Response);
        response.set_op_code(OpCode::Query);
        response.set_recursion_desired(request.recursion_desired());
        response.set_recursion_available(true);
        response.set_response_code(code);

        for query in request.queries() {
            response.add_query(query.clone());
        }

        response.to_bytes().unwrap_or_default()
    }

    pub fn cache(&self) -> &DnsCache {
        &self.cache
    }
}

// Helper: convert our Record to a hickory DNS Record for local zone responses.
// Reuses the conversion logic pattern from microdns-auth.
fn record_to_proto(
    record: &microdns_core::types::Record,
    db: &Db,
) -> Option<hickory_proto::rr::Record> {
    use hickory_proto::rr::rdata::{CNAME, MX, NS, PTR, SOA, SRV, TXT};
    use hickory_proto::rr::{RData, Record as DnsRecord};
    use microdns_core::types::RecordData;
    use std::str::FromStr;

    let fqdn_str = if record.name == "@" {
        let zone = db.find_zone_for_fqdn(&record.name).ok()??;
        ensure_fqdn(&zone.name)
    } else {
        let zone = db.get_zone(&record.zone_id).ok()??;
        format!("{}.{}.", record.name, zone.name)
    };

    let name = Name::from_str(&fqdn_str).ok()?;

    let rdata = match &record.data {
        RecordData::A(addr) => RData::A((*addr).into()),
        RecordData::AAAA(addr) => RData::AAAA((*addr).into()),
        RecordData::CNAME(n) => RData::CNAME(CNAME(Name::from_str(&ensure_fqdn(n)).ok()?)),
        RecordData::MX { preference, exchange } => {
            RData::MX(MX::new(*preference, Name::from_str(&ensure_fqdn(exchange)).ok()?))
        }
        RecordData::NS(n) => RData::NS(NS(Name::from_str(&ensure_fqdn(n)).ok()?)),
        RecordData::PTR(n) => RData::PTR(PTR(Name::from_str(&ensure_fqdn(n)).ok()?)),
        RecordData::SOA(soa) => RData::SOA(SOA::new(
            Name::from_str(&ensure_fqdn(&soa.mname)).ok()?,
            Name::from_str(&ensure_fqdn(&soa.rname)).ok()?,
            soa.serial,
            soa.refresh as i32,
            soa.retry as i32,
            soa.expire as i32,
            soa.minimum,
        )),
        RecordData::SRV(srv) => RData::SRV(SRV::new(
            srv.priority,
            srv.weight,
            srv.port,
            Name::from_str(&ensure_fqdn(&srv.target)).ok()?,
        )),
        RecordData::TXT(text) => RData::TXT(TXT::new(vec![text.clone()])),
        RecordData::CAA(_) => return None, // Simplified for now
    };

    Some(DnsRecord::from_rdata(name, record.ttl, rdata))
}

fn build_soa_record_proto(
    zone: &microdns_core::types::Zone,
) -> Option<hickory_proto::rr::Record> {
    use hickory_proto::rr::rdata::SOA;
    use hickory_proto::rr::{Name, RData, Record as DnsRecord};
    use std::str::FromStr;

    let zone_name = Name::from_str(&ensure_fqdn(&zone.name)).ok()?;
    let mname = Name::from_str(&ensure_fqdn(&zone.soa.mname)).ok()?;
    let rname = Name::from_str(&ensure_fqdn(&zone.soa.rname)).ok()?;

    let rdata = RData::SOA(SOA::new(
        mname,
        rname,
        zone.soa.serial,
        zone.soa.refresh as i32,
        zone.soa.retry as i32,
        zone.soa.expire as i32,
        zone.soa.minimum,
    ));

    let mut record = DnsRecord::from_rdata(zone_name, zone.default_ttl, rdata);
    record.set_record_type(RecordType::SOA);
    Some(record)
}

fn ensure_fqdn(name: &str) -> String {
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{name}.")
    }
}
