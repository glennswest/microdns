use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use microdns_core::types::{Lease, LeaseState};
use redb::ReadableTable;
use serde::Serialize;

/// Leases table (mirrors DHCP crate definition)
const LEASES_TABLE: redb::TableDefinition<&str, &str> = redb::TableDefinition::new("leases");

pub fn router() -> Router<AppState> {
    Router::new().route("/dhcp/status", get(dhcp_status))
}

#[derive(Debug, Clone, Serialize)]
pub struct DhcpPoolSummary {
    pub range_start: String,
    pub range_end: String,
    pub subnet: String,
    pub gateway: String,
    pub domain: String,
    pub lease_time_secs: u64,
    pub pxe_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DhcpStatusConfig {
    pub enabled: bool,
    pub interface: String,
    pub pools: Vec<DhcpPoolSummary>,
    pub reservation_count: usize,
}

#[derive(Serialize)]
struct DhcpStatusResponse {
    enabled: bool,
    interface: String,
    pools: Vec<DhcpPoolSummary>,
    reservation_count: usize,
    active_lease_count: usize,
}

async fn dhcp_status(
    State(state): State<AppState>,
) -> Result<Json<DhcpStatusResponse>, (StatusCode, String)> {
    let dhcp = &state.dhcp_status;

    // Count active leases from DB
    let active_count = match state.db.raw().begin_read() {
        Ok(read_txn) => match read_txn.open_table(LEASES_TABLE) {
            Ok(table) => {
                let now = chrono::Utc::now();
                let mut count = 0usize;
                if let Ok(iter) = table.iter() {
                    for entry in iter {
                        if let Ok(entry) = entry {
                            if let Ok(lease) =
                                serde_json::from_str::<Lease>(entry.1.value())
                            {
                                if lease.state == LeaseState::Active && lease.lease_end > now {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
                count
            }
            Err(_) => 0,
        },
        Err(_) => 0,
    };

    Ok(Json(DhcpStatusResponse {
        enabled: dhcp.enabled,
        interface: dhcp.interface.clone(),
        pools: dhcp.pools.clone(),
        reservation_count: dhcp.reservation_count,
        active_lease_count: active_count,
    }))
}
