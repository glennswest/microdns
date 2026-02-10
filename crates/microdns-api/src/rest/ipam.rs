use crate::security::{internal_error, Pagination};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use microdns_core::types::IpamAllocation;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ipam/pools", get(list_pools))
        .route("/ipam/allocations", get(list_allocations))
        .route("/ipam/allocate", post(allocate))
        .route("/ipam/allocations/{id}", delete(deallocate))
}

#[derive(Serialize)]
struct PoolInfo {
    name: String,
    subnet: String,
    range_start: String,
    range_end: String,
    gateway: String,
    bridge: String,
    total: u32,
    available: u32,
}

#[derive(Deserialize)]
struct AllocateRequest {
    pool: String,
    container: String,
}

#[derive(Serialize)]
struct AllocationResponse {
    id: Uuid,
    ip: String,
    pool: String,
    gateway: String,
    bridge: String,
    subnet: String,
    container: String,
}

fn ip_range_size(start: Ipv4Addr, end: Ipv4Addr) -> u32 {
    let s: u32 = start.into();
    let e: u32 = end.into();
    e.saturating_sub(s) + 1
}

async fn list_pools(
    State(state): State<AppState>,
) -> Result<Json<Vec<PoolInfo>>, (StatusCode, String)> {
    let allocations = state
        .db
        .list_ipam_allocations()
        .map_err(internal_error)?;

    let pools = state
        .ipam_pools
        .iter()
        .map(|p| {
            let start: Ipv4Addr = p.range_start.parse().unwrap_or(Ipv4Addr::UNSPECIFIED);
            let end: Ipv4Addr = p.range_end.parse().unwrap_or(Ipv4Addr::UNSPECIFIED);
            let total = ip_range_size(start, end);
            let used = allocations.iter().filter(|a| a.pool == p.name).count() as u32;
            PoolInfo {
                name: p.name.clone(),
                subnet: p.subnet.clone(),
                range_start: p.range_start.clone(),
                range_end: p.range_end.clone(),
                gateway: p.gateway.clone(),
                bridge: p.bridge.clone(),
                total,
                available: total.saturating_sub(used),
            }
        })
        .collect();

    Ok(Json(pools))
}

async fn list_allocations(
    State(state): State<AppState>,
    Query(page): Query<Pagination>,
) -> Result<Json<Vec<AllocationResponse>>, (StatusCode, String)> {
    let allocations = state
        .db
        .list_ipam_allocations()
        .map_err(internal_error)?;

    let result = allocations
        .into_iter()
        .map(|a| AllocationResponse {
            id: a.id,
            ip: a.ip_addr,
            pool: a.pool,
            gateway: a.gateway,
            bridge: a.bridge,
            subnet: a.subnet,
            container: a.container,
        })
        .collect();

    Ok(Json(page.apply(result)))
}

async fn allocate(
    State(state): State<AppState>,
    Json(req): Json<AllocateRequest>,
) -> Result<(StatusCode, Json<AllocationResponse>), (StatusCode, String)> {
    let pool = state
        .ipam_pools
        .iter()
        .find(|p| p.name == req.pool)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("pool '{}' not found", req.pool),
            )
        })?
        .clone();

    let start: Ipv4Addr = pool
        .range_start
        .parse()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad range_start: {e}")))?;
    let end: Ipv4Addr = pool
        .range_end
        .parse()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad range_end: {e}")))?;

    let allocations = state
        .db
        .list_ipam_allocations()
        .map_err(internal_error)?;

    // Check if container already has an allocation in this pool
    if let Some(existing) = allocations
        .iter()
        .find(|a| a.container == req.container && a.pool == req.pool)
    {
        return Ok((
            StatusCode::OK,
            Json(AllocationResponse {
                id: existing.id,
                ip: existing.ip_addr.clone(),
                pool: existing.pool.clone(),
                gateway: existing.gateway.clone(),
                bridge: existing.bridge.clone(),
                subnet: existing.subnet.clone(),
                container: existing.container.clone(),
            }),
        ));
    }

    let used_ips: std::collections::HashSet<Ipv4Addr> = allocations
        .iter()
        .filter(|a| a.pool == req.pool)
        .filter_map(|a| a.ip_addr.parse().ok())
        .collect();

    let s: u32 = start.into();
    let e: u32 = end.into();

    let mut chosen = None;
    for ip_num in s..=e {
        let ip = Ipv4Addr::from(ip_num);
        if !used_ips.contains(&ip) {
            chosen = Some(ip);
            break;
        }
    }

    let ip = chosen.ok_or_else(|| {
        (
            StatusCode::CONFLICT,
            format!("pool '{}' exhausted", req.pool),
        )
    })?;

    let alloc = IpamAllocation {
        id: Uuid::new_v4(),
        pool: pool.name.clone(),
        ip_addr: ip.to_string(),
        container: req.container.clone(),
        gateway: pool.gateway.clone(),
        bridge: pool.bridge.clone(),
        subnet: pool.subnet.clone(),
        created_at: Utc::now(),
    };

    state
        .db
        .create_ipam_allocation(&alloc)
        .map_err(internal_error)?;

    Ok((
        StatusCode::CREATED,
        Json(AllocationResponse {
            id: alloc.id,
            ip: alloc.ip_addr,
            pool: alloc.pool,
            gateway: alloc.gateway,
            bridge: alloc.bridge,
            subnet: alloc.subnet,
            container: alloc.container,
        }),
    ))
}

async fn deallocate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .db
        .delete_ipam_allocation(&id)
        .map_err(internal_error)?;

    Ok(StatusCode::NO_CONTENT)
}
