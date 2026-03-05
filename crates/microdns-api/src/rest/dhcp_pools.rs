use crate::security::internal_error;
use crate::{AppState, DashboardEvent};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use microdns_core::types::DhcpPool;
use microdns_msg::events::{ChangeAction, Event};
use serde::Deserialize;
use uuid::Uuid;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dhcp/pools", get(list_pools).post(create_pool))
        .route(
            "/dhcp/pools/{id}",
            get(get_pool).patch(patch_pool).delete(delete_pool),
        )
}

#[derive(Deserialize)]
struct CreatePoolRequest {
    name: String,
    range_start: String,
    range_end: String,
    subnet: String,
    gateway: String,
    #[serde(default)]
    dns_servers: Vec<String>,
    #[serde(default)]
    domain: String,
    #[serde(default = "default_lease_time")]
    lease_time_secs: u64,
    #[serde(default)]
    next_server: Option<String>,
    #[serde(default)]
    boot_file: Option<String>,
    #[serde(default)]
    boot_file_efi: Option<String>,
    #[serde(default)]
    ipxe_boot_url: Option<String>,
    #[serde(default)]
    ntp_servers: Option<Vec<String>>,
    #[serde(default)]
    domain_search: Option<Vec<String>>,
    #[serde(default)]
    mtu: Option<u16>,
    #[serde(default)]
    static_routes: Option<Vec<microdns_core::types::StaticRoute>>,
    #[serde(default)]
    log_server: Option<String>,
    #[serde(default)]
    time_offset: Option<i32>,
    #[serde(default)]
    wpad_url: Option<String>,
}

fn default_lease_time() -> u64 {
    3600
}

#[derive(Deserialize)]
struct PatchPoolRequest {
    name: Option<String>,
    range_start: Option<String>,
    range_end: Option<String>,
    subnet: Option<String>,
    gateway: Option<String>,
    dns_servers: Option<Vec<String>>,
    domain: Option<String>,
    lease_time_secs: Option<u64>,
    next_server: Option<Option<String>>,
    boot_file: Option<Option<String>>,
    boot_file_efi: Option<Option<String>>,
    ipxe_boot_url: Option<Option<String>>,
    ntp_servers: Option<Option<Vec<String>>>,
    domain_search: Option<Option<Vec<String>>>,
    mtu: Option<Option<u16>>,
    static_routes: Option<Option<Vec<microdns_core::types::StaticRoute>>>,
    log_server: Option<Option<String>>,
    time_offset: Option<Option<i32>>,
    wpad_url: Option<Option<String>>,
}

async fn list_pools(
    State(state): State<AppState>,
) -> Result<Json<Vec<DhcpPool>>, (StatusCode, String)> {
    let pools = state.db.list_dhcp_pools().map_err(internal_error)?;
    Ok(Json(pools))
}

async fn get_pool(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DhcpPool>, (StatusCode, String)> {
    let pool = state
        .db
        .get_dhcp_pool(&id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "pool not found".to_string()))?;
    Ok(Json(pool))
}

async fn create_pool(
    State(state): State<AppState>,
    Json(req): Json<CreatePoolRequest>,
) -> Result<(StatusCode, Json<DhcpPool>), (StatusCode, String)> {
    let now = Utc::now();
    let pool = DhcpPool {
        id: Uuid::new_v4(),
        name: req.name,
        range_start: req.range_start,
        range_end: req.range_end,
        subnet: req.subnet,
        gateway: req.gateway,
        dns_servers: req.dns_servers,
        domain: req.domain,
        lease_time_secs: req.lease_time_secs,
        next_server: req.next_server,
        boot_file: req.boot_file,
        boot_file_efi: req.boot_file_efi,
        ipxe_boot_url: req.ipxe_boot_url,
        ntp_servers: req.ntp_servers,
        domain_search: req.domain_search,
        mtu: req.mtu,
        static_routes: req.static_routes,
        log_server: req.log_server,
        time_offset: req.time_offset,
        wpad_url: req.wpad_url,
        created_at: now,
        updated_at: now,
    };

    state.db.create_dhcp_pool(&pool).map_err(internal_error)?;

    // Signal DHCP reload
    let _ = state.dhcp_reload_tx.send(());

    // Publish events
    let _ = state.event_tx.send(DashboardEvent::DhcpPoolChanged {
        action: "ADDED".to_string(),
        pool_id: pool.id.to_string(),
        pool_name: pool.name.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::DhcpPoolChanged {
            instance_id: state.instance_id.clone(),
            pool_id: pool.id,
            pool_name: pool.name.clone(),
            action: ChangeAction::Created,
            timestamp: now,
        };
        let _ = bus.publish(&event).await;
    }

    Ok((StatusCode::CREATED, Json(pool)))
}

async fn patch_pool(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchPoolRequest>,
) -> Result<Json<DhcpPool>, (StatusCode, String)> {
    let mut pool = state
        .db
        .get_dhcp_pool(&id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "pool not found".to_string()))?;

    // Apply partial updates — only supplied fields change
    if let Some(v) = req.name { pool.name = v; }
    if let Some(v) = req.range_start { pool.range_start = v; }
    if let Some(v) = req.range_end { pool.range_end = v; }
    if let Some(v) = req.subnet { pool.subnet = v; }
    if let Some(v) = req.gateway { pool.gateway = v; }
    if let Some(v) = req.dns_servers { pool.dns_servers = v; }
    if let Some(v) = req.domain { pool.domain = v; }
    if let Some(v) = req.lease_time_secs { pool.lease_time_secs = v; }
    if let Some(v) = req.next_server { pool.next_server = v; }
    if let Some(v) = req.boot_file { pool.boot_file = v; }
    if let Some(v) = req.boot_file_efi { pool.boot_file_efi = v; }
    if let Some(v) = req.ipxe_boot_url { pool.ipxe_boot_url = v; }
    if let Some(v) = req.ntp_servers { pool.ntp_servers = v; }
    if let Some(v) = req.domain_search { pool.domain_search = v; }
    if let Some(v) = req.mtu { pool.mtu = v; }
    if let Some(v) = req.static_routes { pool.static_routes = v; }
    if let Some(v) = req.log_server { pool.log_server = v; }
    if let Some(v) = req.time_offset { pool.time_offset = v; }
    if let Some(v) = req.wpad_url { pool.wpad_url = v; }
    pool.updated_at = Utc::now();

    state.db.update_dhcp_pool(&pool).map_err(internal_error)?;

    let _ = state.dhcp_reload_tx.send(());
    let _ = state.event_tx.send(DashboardEvent::DhcpPoolChanged {
        action: "MODIFIED".to_string(),
        pool_id: pool.id.to_string(),
        pool_name: pool.name.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::DhcpPoolChanged {
            instance_id: state.instance_id.clone(),
            pool_id: pool.id,
            pool_name: pool.name.clone(),
            action: ChangeAction::Updated,
            timestamp: pool.updated_at,
        };
        let _ = bus.publish(&event).await;
    }

    Ok(Json(pool))
}

async fn delete_pool(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, String)> {
    let pool = state
        .db
        .get_dhcp_pool(&id)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "pool not found".to_string()))?;

    state
        .db
        .delete_dhcp_pool(&id)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let _ = state.dhcp_reload_tx.send(());
    let _ = state.event_tx.send(DashboardEvent::DhcpPoolChanged {
        action: "DELETED".to_string(),
        pool_id: id.to_string(),
        pool_name: pool.name.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::DhcpPoolChanged {
            instance_id: state.instance_id.clone(),
            pool_id: id,
            pool_name: pool.name,
            action: ChangeAction::Deleted,
            timestamp: Utc::now(),
        };
        let _ = bus.publish(&event).await;
    }

    Ok(StatusCode::NO_CONTENT)
}
