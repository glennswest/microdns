use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use microdns_core::types::LeaseState;
use redb::{ReadableTable, TableDefinition};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::{AppState, MAX_WS_CONNECTIONS};

const LEASES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("leases");

/// Maximum serialized message size (2 MB)
const MAX_WS_MESSAGE_SIZE: usize = 2 * 1024 * 1024;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    let current = state.ws_connections.load(Ordering::Relaxed);
    if current >= MAX_WS_CONNECTIONS {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    state.ws_connections.fetch_add(1, Ordering::Relaxed);
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

#[derive(Serialize)]
struct DashboardUpdate {
    zones: Vec<ZoneInfo>,
    leases: Vec<LeaseInfo>,
    instances: Vec<InstanceInfo>,
}

#[derive(Serialize)]
struct ZoneInfo {
    id: String,
    name: String,
    record_count: u64,
}

#[derive(Serialize)]
struct LeaseInfo {
    ip_addr: String,
    mac_addr: String,
    hostname: Option<String>,
    lease_end: String,
}

#[derive(Serialize)]
struct InstanceInfo {
    instance_id: String,
    mode: String,
    healthy: bool,
    active_leases: u64,
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));

    loop {
        interval.tick().await;

        let update = gather_dashboard_data(&state).await;

        let json = match serde_json::to_string(&update) {
            Ok(j) if j.len() <= MAX_WS_MESSAGE_SIZE => j,
            Ok(_) => continue, // skip oversized messages
            Err(_) => continue,
        };

        if socket.send(Message::Text(json.into())).await.is_err() {
            break;
        }
    }

    state.ws_connections.fetch_sub(1, Ordering::Relaxed);
}

async fn gather_dashboard_data(state: &AppState) -> DashboardUpdate {
    // Gather zone info
    let zones = state
        .db
        .get_zone_record_counts()
        .unwrap_or_default()
        .into_iter()
        .map(|(z, count)| ZoneInfo {
            id: z.id.to_string(),
            name: z.name,
            record_count: count as u64,
        })
        .collect();

    // Gather active leases
    let leases = gather_leases(state);

    // Gather instance info
    let instances = if let Some(ref tracker) = state.heartbeat_tracker {
        tracker
            .get_all_status()
            .await
            .into_iter()
            .map(|s| InstanceInfo {
                instance_id: s.instance_id,
                mode: s.mode,
                healthy: s.healthy,
                active_leases: s.active_leases,
            })
            .collect()
    } else {
        vec![]
    };

    DashboardUpdate {
        zones,
        leases,
        instances,
    }
}

fn gather_leases(state: &AppState) -> Vec<LeaseInfo> {
    let read_txn = match state.db.raw().begin_read() {
        Ok(t) => t,
        Err(_) => return vec![],
    };

    let table = match read_txn.open_table(LEASES_TABLE) {
        Ok(t) => t,
        Err(_) => return vec![],
    };

    let now = chrono::Utc::now();
    let mut leases = Vec::new();

    if let Ok(iter) = table.iter() {
        for entry in iter.flatten() {
            if let Ok(lease) =
                serde_json::from_str::<microdns_core::types::Lease>(entry.1.value())
            {
                if lease.state == LeaseState::Active && lease.lease_end > now {
                    leases.push(LeaseInfo {
                        ip_addr: lease.ip_addr,
                        mac_addr: lease.mac_addr,
                        hostname: lease.hostname,
                        lease_end: lease.lease_end.to_rfc3339(),
                    });
                }
            }
        }
    }

    leases
}
