use super::proto;
use microdns_core::db::Db;
use microdns_core::types::{Lease, LeaseState, Record, RecordData, SoaData, Zone};
use microdns_federation::heartbeat::HeartbeatTracker;
use redb::{ReadableTable, TableDefinition};
use std::sync::Arc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

const LEASES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("leases");

pub struct MicroDnsGrpcService {
    db: Db,
    instance_id: String,
    heartbeat_tracker: Option<Arc<HeartbeatTracker>>,
}

impl MicroDnsGrpcService {
    pub fn new(
        db: Db,
        instance_id: &str,
        heartbeat_tracker: Option<Arc<HeartbeatTracker>>,
    ) -> Self {
        Self {
            db,
            instance_id: instance_id.to_string(),
            heartbeat_tracker,
        }
    }
}

fn zone_to_proto(z: &Zone) -> proto::Zone {
    proto::Zone {
        id: z.id.to_string(),
        name: z.name.clone(),
        soa: Some(proto::SoaData {
            mname: z.soa.mname.clone(),
            rname: z.soa.rname.clone(),
            serial: z.soa.serial,
            refresh: z.soa.refresh,
            retry: z.soa.retry,
            expire: z.soa.expire,
            minimum: z.soa.minimum,
        }),
        default_ttl: z.default_ttl,
        created_at: z.created_at.to_rfc3339(),
        updated_at: z.updated_at.to_rfc3339(),
    }
}

fn record_to_proto(r: &Record) -> proto::Record {
    proto::Record {
        id: r.id.to_string(),
        zone_id: r.zone_id.to_string(),
        name: r.name.clone(),
        ttl: r.ttl,
        record_type: r.data.record_type().to_string(),
        data_json: serde_json::to_string(&r.data).unwrap_or_default(),
        enabled: r.enabled,
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

#[tonic::async_trait]
impl proto::zone_service_server::ZoneService for MicroDnsGrpcService {
    async fn list_zones(
        &self,
        _request: Request<proto::ListZonesRequest>,
    ) -> Result<Response<proto::ListZonesResponse>, Status> {
        let zones = self
            .db
            .list_zones()
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(proto::ListZonesResponse {
            zones: zones.iter().map(zone_to_proto).collect(),
        }))
    }

    async fn get_zone(
        &self,
        request: Request<proto::GetZoneRequest>,
    ) -> Result<Response<proto::Zone>, Status> {
        let zone_id: Uuid = request
            .into_inner()
            .zone_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid zone_id"))?;

        let zone = self
            .db
            .get_zone(&zone_id)
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("zone not found"))?;

        Ok(Response::new(zone_to_proto(&zone)))
    }

    async fn create_zone(
        &self,
        request: Request<proto::CreateZoneRequest>,
    ) -> Result<Response<proto::Zone>, Status> {
        let req = request.into_inner();
        let now = chrono::Utc::now();

        let zone = Zone {
            id: Uuid::new_v4(),
            name: req.name,
            soa: SoaData {
                mname: req.mname,
                rname: req.rname,
                serial: 1,
                refresh: 3600,
                retry: 900,
                expire: 604800,
                minimum: 86400,
            },
            default_ttl: if req.default_ttl > 0 {
                req.default_ttl
            } else {
                300
            },
            created_at: now,
            updated_at: now,
        };

        self.db
            .create_zone(&zone.name, &zone)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(zone_to_proto(&zone)))
    }

    async fn delete_zone(
        &self,
        request: Request<proto::DeleteZoneRequest>,
    ) -> Result<Response<proto::DeleteZoneResponse>, Status> {
        let zone_id: Uuid = request
            .into_inner()
            .zone_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid zone_id"))?;

        self.db
            .delete_zone(&zone_id)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(proto::DeleteZoneResponse { success: true }))
    }
}

#[tonic::async_trait]
impl proto::record_service_server::RecordService for MicroDnsGrpcService {
    async fn list_records(
        &self,
        request: Request<proto::ListRecordsRequest>,
    ) -> Result<Response<proto::ListRecordsResponse>, Status> {
        let zone_id: Uuid = request
            .into_inner()
            .zone_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid zone_id"))?;

        let records = self
            .db
            .list_records(&zone_id)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(proto::ListRecordsResponse {
            records: records.iter().map(record_to_proto).collect(),
        }))
    }

    async fn create_record(
        &self,
        request: Request<proto::CreateRecordRequest>,
    ) -> Result<Response<proto::Record>, Status> {
        let req = request.into_inner();
        let zone_id: Uuid = req
            .zone_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid zone_id"))?;

        let data: RecordData = serde_json::from_str(&req.data_json)
            .map_err(|e| Status::invalid_argument(format!("invalid data_json: {e}")))?;

        let now = chrono::Utc::now();
        let record = Record {
            id: Uuid::new_v4(),
            zone_id,
            name: req.name,
            ttl: req.ttl,
            data,
            enabled: req.enabled,
            health_check: None,
            created_at: now,
            updated_at: now,
        };

        self.db
            .create_record(&record)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(record_to_proto(&record)))
    }

    async fn update_record(
        &self,
        request: Request<proto::UpdateRecordRequest>,
    ) -> Result<Response<proto::Record>, Status> {
        let req = request.into_inner();
        let record_id: Uuid = req
            .record_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid record_id"))?;

        let mut record = self
            .db
            .get_record(&record_id)
            .map_err(|e| Status::internal(e.to_string()))?
            .ok_or_else(|| Status::not_found("record not found"))?;

        if !req.name.is_empty() {
            record.name = req.name;
        }
        if req.ttl > 0 {
            record.ttl = req.ttl;
        }
        if !req.data_json.is_empty() {
            record.data = serde_json::from_str(&req.data_json)
                .map_err(|e| Status::invalid_argument(format!("invalid data_json: {e}")))?;
        }
        record.enabled = req.enabled;
        record.updated_at = chrono::Utc::now();

        self.db
            .update_record(&record)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(record_to_proto(&record)))
    }

    async fn delete_record(
        &self,
        request: Request<proto::DeleteRecordRequest>,
    ) -> Result<Response<proto::DeleteRecordResponse>, Status> {
        let record_id: Uuid = request
            .into_inner()
            .record_id
            .parse()
            .map_err(|_| Status::invalid_argument("invalid record_id"))?;

        self.db
            .delete_record(&record_id)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(proto::DeleteRecordResponse { success: true }))
    }
}

#[tonic::async_trait]
impl proto::lease_service_server::LeaseService for MicroDnsGrpcService {
    async fn list_leases(
        &self,
        _request: Request<proto::ListLeasesRequest>,
    ) -> Result<Response<proto::ListLeasesResponse>, Status> {
        let read_txn = self
            .db
            .raw()
            .begin_read()
            .map_err(|e| Status::internal(e.to_string()))?;

        let leases = match read_txn.open_table(LEASES_TABLE) {
            Ok(table) => {
                let now = chrono::Utc::now();
                let mut result = Vec::new();
                let iter = table
                    .iter()
                    .map_err(|e| Status::internal(e.to_string()))?;
                for entry in iter {
                    let entry = entry.map_err(|e| Status::internal(e.to_string()))?;
                    let lease: Lease = serde_json::from_str(entry.1.value())
                        .map_err(|e| Status::internal(e.to_string()))?;
                    if lease.state == LeaseState::Active && lease.lease_end > now {
                        result.push(proto::Lease {
                            id: lease.id.to_string(),
                            ip_addr: lease.ip_addr,
                            mac_addr: lease.mac_addr,
                            hostname: lease.hostname.unwrap_or_default(),
                            lease_start: lease.lease_start.to_rfc3339(),
                            lease_end: lease.lease_end.to_rfc3339(),
                            pool_id: lease.pool_id,
                            state: "active".to_string(),
                        });
                    }
                }
                result
            }
            Err(_) => Vec::new(),
        };

        Ok(Response::new(proto::ListLeasesResponse { leases }))
    }
}

#[tonic::async_trait]
impl proto::cluster_service_server::ClusterService for MicroDnsGrpcService {
    async fn get_cluster_status(
        &self,
        _request: Request<proto::ClusterStatusRequest>,
    ) -> Result<Response<proto::ClusterStatusResponse>, Status> {
        let instances = if let Some(ref tracker) = self.heartbeat_tracker {
            tracker
                .get_all_status()
                .await
                .into_iter()
                .map(|s| proto::InstanceInfo {
                    instance_id: s.instance_id,
                    mode: s.mode,
                    uptime_secs: s.uptime_secs,
                    active_leases: s.active_leases,
                    zones_served: s.zones_served,
                    last_seen: s.last_seen.to_rfc3339(),
                    healthy: s.healthy,
                })
                .collect()
        } else {
            vec![]
        };

        Ok(Response::new(proto::ClusterStatusResponse {
            instance_id: self.instance_id.clone(),
            instances,
        }))
    }

    async fn heartbeat(
        &self,
        request: Request<proto::HeartbeatRequest>,
    ) -> Result<Response<proto::HeartbeatResponse>, Status> {
        let req = request.into_inner();

        if let Some(ref tracker) = self.heartbeat_tracker {
            tracker
                .record_heartbeat(
                    &req.instance_id,
                    &req.mode,
                    req.uptime_secs,
                    req.active_leases,
                    req.zones_served,
                )
                .await;
        }

        Ok(Response::new(proto::HeartbeatResponse {
            acknowledged: true,
        }))
    }

    async fn push_config(
        &self,
        _request: Request<proto::PushConfigRequest>,
    ) -> Result<Response<proto::PushConfigResponse>, Status> {
        // Config push would be handled by the federation coordinator
        Ok(Response::new(proto::PushConfigResponse { success: true }))
    }
}

#[tonic::async_trait]
impl proto::health_service_server::HealthService for MicroDnsGrpcService {
    async fn get_health_status(
        &self,
        _request: Request<proto::HealthStatusRequest>,
    ) -> Result<Response<proto::HealthStatusResponse>, Status> {
        // Get all records with health checks and their current status
        let zones = self
            .db
            .list_zones()
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut records = Vec::new();
        for zone in &zones {
            let zone_records = self
                .db
                .list_records(&zone.id)
                .map_err(|e| Status::internal(e.to_string()))?;

            for record in &zone_records {
                if record.health_check.is_some() {
                    records.push(proto::RecordHealth {
                        record_id: record.id.to_string(),
                        record_name: record.name.clone(),
                        zone_name: zone.name.clone(),
                        healthy: record.enabled,
                        success_count: 0,
                        failure_count: 0,
                    });
                }
            }
        }

        Ok(Response::new(proto::HealthStatusResponse { records }))
    }
}
