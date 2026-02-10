use crate::zone::{build_soa_record, from_rdata, to_rdata};
use chrono::Utc;
use hickory_proto::op::{Message, MessageType, OpCode, Query};
use hickory_proto::rr::{Name, RData, Record as DnsRecord, RecordType};
use hickory_proto::serialize::binary::{BinDecodable, BinEncodable};
use microdns_core::db::Db;
use microdns_core::types::{Record, SoaData, Zone};
use std::net::SocketAddr;
use std::str::FromStr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, info};
use uuid::Uuid;

/// Maximum number of records accepted via AXFR
const MAX_AXFR_RECORDS: usize = 100_000;

/// Maximum total bytes read during AXFR
const MAX_AXFR_BYTES: usize = 100 * 1024 * 1024;

pub struct ZoneTransfer {
    db: Db,
}

#[derive(Debug, serde::Serialize)]
pub struct TransferResult {
    pub zone_name: String,
    pub records_imported: usize,
}

impl ZoneTransfer {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Outbound: build AXFR response records for a zone (SOA, records..., SOA).
    pub fn build_axfr_records(&self, zone_name: &str) -> anyhow::Result<Vec<DnsRecord>> {
        let zone_name = zone_name.trim_end_matches('.');
        let zone = self
            .db
            .find_zone_for_fqdn(zone_name)?
            .ok_or_else(|| anyhow::anyhow!("zone not found: {zone_name}"))?;

        // Ensure zone name matches exactly (not a subdomain match)
        if zone.name.trim_end_matches('.') != zone_name {
            return Err(anyhow::anyhow!("zone not found: {zone_name}"));
        }

        let soa = build_soa_record(&zone)
            .ok_or_else(|| anyhow::anyhow!("failed to build SOA for {zone_name}"))?;

        let mut result = Vec::new();
        result.push(soa.clone());

        let records = self.db.list_records(&zone.id)?;
        let zone_fqdn = format!("{}.", zone.name);

        for record in &records {
            let fqdn = if record.name == "@" {
                zone_fqdn.clone()
            } else {
                format!("{}.{}", record.name, zone_fqdn)
            };

            let name = match Name::from_str(&fqdn) {
                Ok(n) => n,
                Err(_) => continue,
            };

            if let Some(rdata) = to_rdata(&record.data) {
                let dns_record = DnsRecord::from_rdata(name, record.ttl, rdata);
                result.push(dns_record);
            }
        }

        result.push(soa);
        Ok(result)
    }

    /// Inbound: pull zone via AXFR from remote primary.
    pub async fn axfr_pull(
        &self,
        zone_name: &str,
        primary: SocketAddr,
    ) -> anyhow::Result<TransferResult> {
        let zone_name = zone_name.trim_end_matches('.');
        info!("AXFR pull: {zone_name} from {primary}");

        // TCP connect
        let mut stream = TcpStream::connect(primary).await?;

        // Build AXFR query
        let qname = Name::from_str(&format!("{zone_name}."))?;
        let mut query = Query::new();
        query.set_name(qname.clone());
        query.set_query_type(RecordType::AXFR);

        let mut msg = Message::new();
        msg.set_id(rand_id());
        msg.set_message_type(MessageType::Query);
        msg.set_op_code(OpCode::Query);
        msg.set_recursion_desired(false);
        msg.add_query(query);

        let wire = msg.to_bytes()?;

        // Send with 2-byte BE length prefix
        let len = wire.len() as u16;
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&wire).await?;
        stream.flush().await?;

        // Read responses
        let mut soa_count = 0u32;
        let mut parsed_records: Vec<(String, microdns_core::types::RecordData, u32)> = Vec::new();
        let mut soa_data: Option<SoaData> = None;
        let mut default_ttl: u32 = 300;
        let mut total_bytes: usize = 0;

        loop {
            // Read 2-byte length
            let msg_len = match stream.read_u16().await {
                Ok(l) => l as usize,
                Err(e) => {
                    if soa_count >= 2 {
                        break;
                    }
                    return Err(anyhow::anyhow!("read error during AXFR: {e}"));
                }
            };

            if msg_len == 0 {
                break;
            }

            total_bytes += msg_len;
            if total_bytes > MAX_AXFR_BYTES {
                return Err(anyhow::anyhow!(
                    "AXFR exceeded max size ({MAX_AXFR_BYTES} bytes)"
                ));
            }

            let mut buf = vec![0u8; msg_len];
            stream.read_exact(&mut buf).await?;

            let response = Message::from_bytes(&buf)?;

            if response.response_code() != hickory_proto::op::ResponseCode::NoError {
                return Err(anyhow::anyhow!(
                    "AXFR error: {:?}",
                    response.response_code()
                ));
            }

            for answer in response.answers() {
                let Some(rdata) = answer.data() else {
                    continue;
                };
                match rdata {
                    RData::SOA(soa) => {
                        soa_count += 1;
                        if soa_data.is_none() {
                            default_ttl = answer.ttl();
                            soa_data = Some(SoaData {
                                mname: soa.mname().to_string().trim_end_matches('.').to_string(),
                                rname: soa.rname().to_string().trim_end_matches('.').to_string(),
                                serial: soa.serial(),
                                refresh: soa.refresh() as u32,
                                retry: soa.retry() as u32,
                                expire: soa.expire() as u32,
                                minimum: soa.minimum(),
                            });
                        }
                        if soa_count >= 2 {
                            break;
                        }
                    }
                    rdata => {
                        if parsed_records.len() >= MAX_AXFR_RECORDS {
                            return Err(anyhow::anyhow!(
                                "AXFR exceeded max record count ({MAX_AXFR_RECORDS})"
                            ));
                        }
                        if let Some((rel_name, data)) =
                            from_rdata(rdata, answer.name(), zone_name)
                        {
                            parsed_records.push((rel_name, data, answer.ttl()));
                        } else {
                            debug!(
                                "skipping unsupported record: {} {:?}",
                                answer.name(),
                                answer.record_type()
                            );
                        }
                    }
                }
            }

            if soa_count >= 2 {
                break;
            }
        }

        let soa = soa_data.ok_or_else(|| anyhow::anyhow!("no SOA in AXFR response"))?;
        info!(
            "AXFR {zone_name}: received {} records, serial {}",
            parsed_records.len(),
            soa.serial
        );

        // Create or update zone
        let zone = match self.db.get_zone_by_name(zone_name)? {
            Some(mut existing) => {
                existing.soa = soa;
                existing.default_ttl = default_ttl;
                existing.updated_at = Utc::now();
                // Update zone in db via delete + recreate (redb has no update_zone)
                let zone_id = existing.id;
                self.db.delete_zone_records(&zone_id)?;
                // Update zone SOA by delete/recreate
                self.db.delete_zone(&zone_id)?;
                let zone = Zone {
                    id: zone_id,
                    name: zone_name.to_string(),
                    soa: existing.soa,
                    default_ttl: existing.default_ttl,
                    created_at: existing.created_at,
                    updated_at: existing.updated_at,
                };
                self.db.create_zone(zone_name, &zone)?;
                zone
            }
            None => {
                let zone = Zone {
                    id: Uuid::new_v4(),
                    name: zone_name.to_string(),
                    soa,
                    default_ttl,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                };
                self.db.create_zone(zone_name, &zone)?;
                zone
            }
        };

        // Import records
        let count = parsed_records.len();
        for (name, data, ttl) in parsed_records {
            let record = Record {
                id: Uuid::new_v4(),
                zone_id: zone.id,
                name,
                ttl,
                data,
                enabled: true,
                health_check: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            self.db.create_record(&record)?;
        }

        info!("AXFR {zone_name}: imported {count} records");
        Ok(TransferResult {
            zone_name: zone_name.to_string(),
            records_imported: count,
        })
    }
}

fn rand_id() -> u16 {
    use std::time::SystemTime;
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (t.subsec_nanos() & 0xFFFF) as u16
}
