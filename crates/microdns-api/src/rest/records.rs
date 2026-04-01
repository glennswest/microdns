use crate::security::{internal_error, validate_dns_name, Pagination};
use crate::{AppState, DashboardEvent};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use microdns_core::reverse;
use microdns_core::types::{HealthCheck, Record, RecordData};
use microdns_msg::events::{ChangeAction, Event};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/zones/{zone_id}/records",
            get(list_records).post(create_record),
        )
        .route(
            "/zones/{zone_id}/records/{record_id}",
            get(get_record).put(update_record).delete(delete_record),
        )
}

#[derive(Serialize)]
struct RecordResponse {
    id: Uuid,
    zone_id: Uuid,
    name: String,
    ttl: u32,
    #[serde(rename = "type")]
    record_type: String,
    data: RecordData,
    enabled: bool,
    health_check: Option<HealthCheck>,
    created_at: String,
    updated_at: String,
}

impl RecordResponse {
    fn from_record(r: Record) -> Self {
        Self {
            id: r.id,
            zone_id: r.zone_id,
            name: r.name,
            ttl: r.ttl,
            record_type: r.data.record_type().to_string(),
            data: r.data,
            enabled: r.enabled,
            health_check: r.health_check,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Deserialize)]
struct CreateRecordRequest {
    name: String,
    #[serde(default = "default_ttl")]
    ttl: u32,
    data: RecordData,
    #[serde(default = "default_true")]
    enabled: bool,
    health_check: Option<HealthCheck>,
}

#[derive(Deserialize)]
struct UpdateRecordRequest {
    name: Option<String>,
    ttl: Option<u32>,
    data: Option<RecordData>,
    enabled: Option<bool>,
    health_check: Option<Option<HealthCheck>>,
}

fn default_ttl() -> u32 {
    300
}

fn default_true() -> bool {
    true
}

async fn list_records(
    State(state): State<AppState>,
    Path(zone_id): Path<Uuid>,
    Query(page): Query<Pagination>,
) -> Result<Json<Vec<RecordResponse>>, (StatusCode, String)> {
    // Verify zone exists
    state
        .db
        .get_zone(&zone_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "zone not found".to_string()))?;

    let records = state
        .db
        .list_records(&zone_id)
        .map_err(internal_error)?;

    let response: Vec<RecordResponse> = records.into_iter().map(RecordResponse::from_record).collect();

    Ok(Json(page.apply(response)))
}

async fn create_record(
    State(state): State<AppState>,
    Path(zone_id): Path<Uuid>,
    Json(req): Json<CreateRecordRequest>,
) -> Result<(StatusCode, Json<RecordResponse>), (StatusCode, String)> {
    // Verify zone exists and get zone name for reverse sync
    let zone = state
        .db
        .get_zone(&zone_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "zone not found".to_string()))?;

    validate_dns_name(&req.name).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Dedup: if an identical record (same name + type + data) already exists,
    // return it instead of creating a duplicate.
    let existing = state
        .db
        .query_records(&zone_id, &req.name, req.data.record_type())
        .map_err(internal_error)?;
    for rec in &existing {
        if rec.data == req.data {
            return Ok((StatusCode::OK, Json(RecordResponse::from_record(rec.clone()))));
        }
    }

    let record = Record {
        id: Uuid::new_v4(),
        zone_id,
        name: req.name,
        ttl: req.ttl,
        data: req.data,
        enabled: req.enabled,
        health_check: req.health_check,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    state
        .db
        .create_record(&record)
        .map_err(internal_error)?;

    // Increment SOA serial
    let _ = state.db.increment_soa_serial(&zone_id);

    // Auto-sync reverse PTR for A/AAAA records
    if let Err(e) = reverse::sync_reverse_record(
        &state.db,
        &record.name,
        &zone.name,
        &record.data,
        record.ttl,
    ) {
        tracing::warn!("reverse zone sync failed for {}: {e}", record.name);
    }

    // Invalidate recursor cache so new records are served immediately
    if let Some(ref cache) = state.recursor_cache {
        cache.clear();
    }

    let _ = state.event_tx.send(DashboardEvent::RecordChanged {
        action: "ADDED".to_string(),
        zone_id: zone_id.to_string(),
        record_name: record.name.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::RecordChanged {
            instance_id: state.instance_id.clone(),
            zone_id,
            record_id: record.id,
            record_name: record.name.clone(),
            action: ChangeAction::Created,
            timestamp: Utc::now(),
        };
        let _ = bus.publish(&event).await;
    }

    Ok((
        StatusCode::CREATED,
        Json(RecordResponse::from_record(record)),
    ))
}

async fn get_record(
    State(state): State<AppState>,
    Path((_zone_id, record_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<RecordResponse>, (StatusCode, String)> {
    let record = state
        .db
        .get_record(&record_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "record not found".to_string()))?;

    Ok(Json(RecordResponse::from_record(record)))
}

async fn update_record(
    State(state): State<AppState>,
    Path((zone_id, record_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateRecordRequest>,
) -> Result<Json<RecordResponse>, (StatusCode, String)> {
    let mut record = state
        .db
        .get_record(&record_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "record not found".to_string()))?;

    // Capture old values for reverse zone cleanup
    let old_name = record.name.clone();
    let old_data = record.data.clone();

    if let Some(ref name) = req.name {
        validate_dns_name(name).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        record.name = name.clone();
    }
    if let Some(ttl) = req.ttl {
        record.ttl = ttl;
    }
    if let Some(data) = req.data {
        record.data = data;
    }
    if let Some(enabled) = req.enabled {
        record.enabled = enabled;
    }
    if let Some(health_check) = req.health_check {
        record.health_check = health_check;
    }
    record.updated_at = Utc::now();

    state
        .db
        .update_record(&record)
        .map_err(internal_error)?;

    let _ = state.db.increment_soa_serial(&zone_id);

    // Update reverse PTR if name or data changed
    if old_name != record.name || old_data != record.data {
        if let Ok(Some(zone)) = state.db.get_zone(&zone_id) {
            // Delete old PTR
            if let Err(e) = reverse::delete_reverse_record(
                &state.db,
                &old_name,
                &zone.name,
                &old_data,
            ) {
                tracing::warn!("reverse zone cleanup failed for old {old_name}: {e}");
            }
            // Create new PTR
            if let Err(e) = reverse::sync_reverse_record(
                &state.db,
                &record.name,
                &zone.name,
                &record.data,
                record.ttl,
            ) {
                tracing::warn!("reverse zone sync failed for {}: {e}", record.name);
            }
        }
    }

    // Invalidate recursor cache so updated records are served immediately
    if let Some(ref cache) = state.recursor_cache {
        cache.clear();
    }

    let _ = state.event_tx.send(DashboardEvent::RecordChanged {
        action: "MODIFIED".to_string(),
        zone_id: zone_id.to_string(),
        record_name: record.name.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::RecordChanged {
            instance_id: state.instance_id.clone(),
            zone_id,
            record_id: record.id,
            record_name: record.name.clone(),
            action: ChangeAction::Updated,
            timestamp: Utc::now(),
        };
        let _ = bus.publish(&event).await;
    }

    Ok(Json(RecordResponse::from_record(record)))
}

async fn delete_record(
    State(state): State<AppState>,
    Path((zone_id, record_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let record = state
        .db
        .get_record(&record_id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "record not found".to_string()))?;

    state
        .db
        .delete_record(&record_id)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let _ = state.db.increment_soa_serial(&zone_id);

    // Delete reverse PTR for A/AAAA records
    if let Ok(Some(zone)) = state.db.get_zone(&zone_id) {
        if let Err(e) = reverse::delete_reverse_record(
            &state.db,
            &record.name,
            &zone.name,
            &record.data,
        ) {
            tracing::warn!("reverse zone cleanup failed for {}: {e}", record.name);
        }
    }

    // Invalidate recursor cache so deleted records stop resolving immediately
    if let Some(ref cache) = state.recursor_cache {
        cache.clear();
    }

    let _ = state.event_tx.send(DashboardEvent::RecordChanged {
        action: "DELETED".to_string(),
        zone_id: zone_id.to_string(),
        record_name: record.name,
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::RecordChanged {
            instance_id: state.instance_id.clone(),
            zone_id,
            record_id,
            record_name: String::new(),
            action: ChangeAction::Deleted,
            timestamp: Utc::now(),
        };
        let _ = bus.publish(&event).await;
    }

    Ok(StatusCode::NO_CONTENT)
}
