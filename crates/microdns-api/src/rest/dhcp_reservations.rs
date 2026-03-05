use crate::security::internal_error;
use crate::{AppState, DashboardEvent};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use microdns_core::types::DhcpDbReservation;
use microdns_msg::events::{ChangeAction, Event};
use serde::Deserialize;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dhcp/reservations", get(list_reservations).post(create_reservation))
        .route(
            "/dhcp/reservations/{mac}",
            get(get_reservation).patch(patch_reservation).delete(delete_reservation),
        )
}

#[derive(Deserialize)]
struct CreateReservationRequest {
    mac: String,
    ip: String,
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default)]
    gateway: Option<String>,
    #[serde(default)]
    dns_servers: Option<Vec<String>>,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    ntp_servers: Option<Vec<String>>,
    #[serde(default)]
    domain_search: Option<Vec<String>>,
    #[serde(default)]
    mtu: Option<u16>,
    #[serde(default)]
    next_server: Option<String>,
    #[serde(default)]
    boot_file: Option<String>,
    #[serde(default)]
    boot_file_efi: Option<String>,
    #[serde(default)]
    ipxe_boot_url: Option<String>,
    #[serde(default)]
    root_path: Option<String>,
    #[serde(default)]
    static_routes: Option<Vec<microdns_core::types::StaticRoute>>,
    #[serde(default)]
    log_server: Option<String>,
    #[serde(default)]
    time_offset: Option<i32>,
    #[serde(default)]
    wpad_url: Option<String>,
    #[serde(default)]
    lease_time_secs: Option<u64>,
}

#[derive(Deserialize)]
struct PatchReservationRequest {
    ip: Option<String>,
    hostname: Option<Option<String>>,
    gateway: Option<Option<String>>,
    dns_servers: Option<Option<Vec<String>>>,
    domain: Option<Option<String>>,
    ntp_servers: Option<Option<Vec<String>>>,
    domain_search: Option<Option<Vec<String>>>,
    mtu: Option<Option<u16>>,
    next_server: Option<Option<String>>,
    boot_file: Option<Option<String>>,
    boot_file_efi: Option<Option<String>>,
    ipxe_boot_url: Option<Option<String>>,
    root_path: Option<Option<String>>,
    static_routes: Option<Option<Vec<microdns_core::types::StaticRoute>>>,
    log_server: Option<Option<String>>,
    time_offset: Option<Option<i32>>,
    wpad_url: Option<Option<String>>,
    lease_time_secs: Option<Option<u64>>,
}

/// Normalize MAC: accept colon or dash separated, return lowercase colon-separated.
fn normalize_mac(mac: &str) -> String {
    mac.to_lowercase().replace('-', ":")
}

async fn list_reservations(
    State(state): State<AppState>,
) -> Result<Json<Vec<DhcpDbReservation>>, (StatusCode, String)> {
    let reservations = state.db.list_dhcp_reservations().map_err(internal_error)?;
    Ok(Json(reservations))
}

async fn get_reservation(
    State(state): State<AppState>,
    Path(mac): Path<String>,
) -> Result<Json<DhcpDbReservation>, (StatusCode, String)> {
    let mac = normalize_mac(&mac);
    let res = state
        .db
        .get_dhcp_reservation(&mac)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "reservation not found".to_string()))?;
    Ok(Json(res))
}

async fn create_reservation(
    State(state): State<AppState>,
    Json(req): Json<CreateReservationRequest>,
) -> Result<(StatusCode, Json<DhcpDbReservation>), (StatusCode, String)> {
    let now = Utc::now();
    let mac = normalize_mac(&req.mac);
    let res = DhcpDbReservation {
        mac: mac.clone(),
        ip: req.ip,
        hostname: req.hostname,
        gateway: req.gateway,
        dns_servers: req.dns_servers,
        domain: req.domain,
        ntp_servers: req.ntp_servers,
        domain_search: req.domain_search,
        mtu: req.mtu,
        next_server: req.next_server,
        boot_file: req.boot_file,
        boot_file_efi: req.boot_file_efi,
        ipxe_boot_url: req.ipxe_boot_url,
        root_path: req.root_path,
        static_routes: req.static_routes,
        log_server: req.log_server,
        time_offset: req.time_offset,
        wpad_url: req.wpad_url,
        lease_time_secs: req.lease_time_secs,
        created_at: now,
        updated_at: now,
    };

    state
        .db
        .create_dhcp_reservation(&res)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;

    let _ = state.event_tx.send(DashboardEvent::DhcpReservationChanged {
        action: "ADDED".to_string(),
        mac: res.mac.clone(),
        ip: res.ip.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::DhcpReservationChanged {
            instance_id: state.instance_id.clone(),
            mac: res.mac.clone(),
            ip: res.ip.clone(),
            hostname: res.hostname.clone(),
            action: ChangeAction::Created,
            timestamp: now,
        };
        let _ = bus.publish(&event).await;
    }

    Ok((StatusCode::CREATED, Json(res)))
}

async fn patch_reservation(
    State(state): State<AppState>,
    Path(mac): Path<String>,
    Json(req): Json<PatchReservationRequest>,
) -> Result<Json<DhcpDbReservation>, (StatusCode, String)> {
    let mac = normalize_mac(&mac);
    let mut res = state
        .db
        .get_dhcp_reservation(&mac)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "reservation not found".to_string()))?;

    if let Some(v) = req.ip { res.ip = v; }
    if let Some(v) = req.hostname { res.hostname = v; }
    if let Some(v) = req.gateway { res.gateway = v; }
    if let Some(v) = req.dns_servers { res.dns_servers = v; }
    if let Some(v) = req.domain { res.domain = v; }
    if let Some(v) = req.ntp_servers { res.ntp_servers = v; }
    if let Some(v) = req.domain_search { res.domain_search = v; }
    if let Some(v) = req.mtu { res.mtu = v; }
    if let Some(v) = req.next_server { res.next_server = v; }
    if let Some(v) = req.boot_file { res.boot_file = v; }
    if let Some(v) = req.boot_file_efi { res.boot_file_efi = v; }
    if let Some(v) = req.ipxe_boot_url { res.ipxe_boot_url = v; }
    if let Some(v) = req.root_path { res.root_path = v; }
    if let Some(v) = req.static_routes { res.static_routes = v; }
    if let Some(v) = req.log_server { res.log_server = v; }
    if let Some(v) = req.time_offset { res.time_offset = v; }
    if let Some(v) = req.wpad_url { res.wpad_url = v; }
    if let Some(v) = req.lease_time_secs { res.lease_time_secs = v; }
    res.updated_at = Utc::now();

    state.db.update_dhcp_reservation(&res).map_err(internal_error)?;

    let _ = state.event_tx.send(DashboardEvent::DhcpReservationChanged {
        action: "MODIFIED".to_string(),
        mac: res.mac.clone(),
        ip: res.ip.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::DhcpReservationChanged {
            instance_id: state.instance_id.clone(),
            mac: res.mac.clone(),
            ip: res.ip.clone(),
            hostname: res.hostname.clone(),
            action: ChangeAction::Updated,
            timestamp: res.updated_at,
        };
        let _ = bus.publish(&event).await;
    }

    Ok(Json(res))
}

async fn delete_reservation(
    State(state): State<AppState>,
    Path(mac): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let mac = normalize_mac(&mac);
    let res = state
        .db
        .get_dhcp_reservation(&mac)
        .map_err(internal_error)?
        .ok_or((StatusCode::NOT_FOUND, "reservation not found".to_string()))?;

    state
        .db
        .delete_dhcp_reservation(&mac)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let _ = state.event_tx.send(DashboardEvent::DhcpReservationChanged {
        action: "DELETED".to_string(),
        mac: mac.clone(),
        ip: res.ip.clone(),
    });
    if let Some(ref bus) = state.message_bus {
        let event = Event::DhcpReservationChanged {
            instance_id: state.instance_id.clone(),
            mac,
            ip: res.ip,
            hostname: res.hostname,
            action: ChangeAction::Deleted,
            timestamp: Utc::now(),
        };
        let _ = bus.publish(&event).await;
    }

    Ok(StatusCode::NO_CONTENT)
}
