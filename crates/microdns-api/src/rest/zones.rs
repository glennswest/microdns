use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use microdns_core::types::{SoaData, Zone};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/zones", get(list_zones).post(create_zone))
        .route("/zones/{id}", get(get_zone).delete(delete_zone))
        .route("/zones/transfer", post(transfer_zone))
}

#[derive(Serialize)]
struct ZoneResponse {
    id: Uuid,
    name: String,
    soa: SoaData,
    default_ttl: u32,
    record_count: Option<usize>,
    created_at: String,
    updated_at: String,
}

impl ZoneResponse {
    fn from_zone(zone: Zone, record_count: Option<usize>) -> Self {
        Self {
            id: zone.id,
            name: zone.name,
            soa: zone.soa,
            default_ttl: zone.default_ttl,
            record_count,
            created_at: zone.created_at.to_rfc3339(),
            updated_at: zone.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Deserialize)]
struct CreateZoneRequest {
    name: String,
    #[serde(default = "default_ttl")]
    default_ttl: u32,
    #[serde(default)]
    soa: Option<CreateSoaRequest>,
}

#[derive(Deserialize)]
struct CreateSoaRequest {
    mname: Option<String>,
    rname: Option<String>,
    refresh: Option<u32>,
    retry: Option<u32>,
    expire: Option<u32>,
    minimum: Option<u32>,
}

fn default_ttl() -> u32 {
    300
}

async fn list_zones(
    State(state): State<AppState>,
) -> Result<Json<Vec<ZoneResponse>>, (StatusCode, String)> {
    let zones = state
        .db
        .get_zone_record_counts()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<ZoneResponse> = zones
        .into_iter()
        .map(|(zone, count)| ZoneResponse::from_zone(zone, Some(count)))
        .collect();

    Ok(Json(response))
}

async fn create_zone(
    State(state): State<AppState>,
    Json(req): Json<CreateZoneRequest>,
) -> Result<(StatusCode, Json<ZoneResponse>), (StatusCode, String)> {
    let name = req.name.trim_end_matches('.').to_string();

    let soa = match req.soa {
        Some(s) => SoaData {
            mname: s.mname.unwrap_or_else(|| format!("ns1.{name}")),
            rname: s.rname.unwrap_or_else(|| format!("admin.{name}")),
            serial: Utc::now().format("%Y%m%d00").to_string().parse().unwrap_or(1),
            refresh: s.refresh.unwrap_or(3600),
            retry: s.retry.unwrap_or(900),
            expire: s.expire.unwrap_or(604800),
            minimum: s.minimum.unwrap_or(300),
        },
        None => SoaData {
            mname: format!("ns1.{name}"),
            rname: format!("admin.{name}"),
            serial: Utc::now().format("%Y%m%d00").to_string().parse().unwrap_or(1),
            refresh: 3600,
            retry: 900,
            expire: 604800,
            minimum: 300,
        },
    };

    let zone = Zone {
        id: Uuid::new_v4(),
        name: name.clone(),
        soa,
        default_ttl: req.default_ttl,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };

    state
        .db
        .create_zone(&name, &zone)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(ZoneResponse::from_zone(zone, Some(0))),
    ))
}

async fn get_zone(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ZoneResponse>, (StatusCode, String)> {
    let zone = state
        .db
        .get_zone(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "zone not found".to_string()))?;

    let records = state
        .db
        .list_records(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ZoneResponse::from_zone(zone, Some(records.len()))))
}

async fn delete_zone(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .db
        .delete_zone(&id)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TransferRequest {
    zone: String,
    primary: String,
}

#[derive(Serialize)]
struct TransferResponse {
    zone_name: String,
    records_imported: usize,
}

async fn transfer_zone(
    State(state): State<AppState>,
    Json(req): Json<TransferRequest>,
) -> Result<Json<TransferResponse>, (StatusCode, String)> {
    let primary: std::net::SocketAddr = req
        .primary
        .parse()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid primary address: {e}")))?;

    let zt = microdns_auth::transfer::ZoneTransfer::new(state.db.clone());
    let result = zt
        .axfr_pull(&req.zone, primary)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("AXFR failed: {e}")))?;

    Ok(Json(TransferResponse {
        zone_name: result.zone_name,
        records_imported: result.records_imported,
    }))
}
