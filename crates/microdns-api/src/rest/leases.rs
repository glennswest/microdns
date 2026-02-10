use crate::security::{internal_error, Pagination};
use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use microdns_core::types::{Lease, LeaseState};
use redb::{ReadableTable, TableDefinition};
use serde::Serialize;
use uuid::Uuid;

/// Leases table (mirrors DHCP crate definition)
const LEASES_TABLE: TableDefinition<&str, &str> = TableDefinition::new("leases");

pub fn router() -> Router<AppState> {
    Router::new().route("/leases", get(list_leases))
}

#[derive(Serialize)]
struct LeaseResponse {
    id: Uuid,
    ip_addr: String,
    mac_addr: String,
    hostname: Option<String>,
    lease_start: String,
    lease_end: String,
    pool_id: String,
    state: LeaseState,
}

impl LeaseResponse {
    fn from_lease(l: Lease) -> Self {
        Self {
            id: l.id,
            ip_addr: l.ip_addr,
            mac_addr: l.mac_addr,
            hostname: l.hostname,
            lease_start: l.lease_start.to_rfc3339(),
            lease_end: l.lease_end.to_rfc3339(),
            pool_id: l.pool_id,
            state: l.state,
        }
    }
}

async fn list_leases(
    State(state): State<AppState>,
    Query(page): Query<Pagination>,
) -> Result<Json<Vec<LeaseResponse>>, (StatusCode, String)> {
    let read_txn = state
        .db
        .raw()
        .begin_read()
        .map_err(internal_error)?;

    let leases = match read_txn.open_table(LEASES_TABLE) {
        Ok(table) => {
            let now = chrono::Utc::now();
            let mut result = Vec::new();
            let iter = table
                .iter()
                .map_err(|e: redb::StorageError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            for entry in iter {
                let entry =
                    entry.map_err(|e: redb::StorageError| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                let lease: Lease = serde_json::from_str(entry.1.value())
                    .map_err(internal_error)?;
                if lease.state == LeaseState::Active && lease.lease_end > now {
                    result.push(LeaseResponse::from_lease(lease));
                }
            }
            result
        }
        Err(_) => Vec::new(), // Table doesn't exist yet = no leases
    };

    Ok(Json(page.apply(leases)))
}
